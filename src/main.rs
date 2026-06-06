use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use chrono::Utc;
use clap::{Parser, Subcommand};
use hmac::{Hmac, Mac};
use reqwest::Url;
use reqwest::blocking::{Client, multipart};
use reqwest::header::{
    ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const LINKHUB_VERSION: &str = "2.0";
const POPBILL_VERSION: &str = "1.0";
const SERVICE_ID_REAL: &str = "POPBILL";
const SERVICE_ID_TEST: &str = "POPBILL_TEST";

#[derive(Parser, Debug)]
#[command(name = "popbill-fax")]
#[command(about = "Popbill FAX CLI using the Linkhub auth protocol")]
struct Cli {
    #[arg(long, default_value = ".env", global = true)]
    env: PathBuf,
    #[arg(long, default_value = "config.toml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    DryRun {
        #[arg(long)]
        request: PathBuf,
    },
    Send {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        confirm_send: bool,
    },
    List {
        #[arg(long)]
        start_date: Option<String>,
        #[arg(long)]
        end_date: Option<String>,
        #[arg(long, value_delimiter = ',')]
        state: Vec<String>,
        #[arg(long)]
        reserve_yn: Option<bool>,
        #[arg(long)]
        sender_only: Option<bool>,
        #[arg(long, default_value = "D")]
        order: String,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long, default_value_t = 20)]
        per_page: u32,
        #[arg(long)]
        query: Option<String>,
    },
    Read {
        #[arg(long)]
        receipt_num: Option<String>,
        #[arg(long)]
        request_num: Option<String>,
    },
    Delete {
        #[arg(long)]
        receipt_num: Option<String>,
        #[arg(long)]
        request_num: Option<String>,
        #[arg(long)]
        confirm_delete: bool,
    },
    SenderList,
    CheckSender {
        #[arg(long)]
        sender: String,
    },
    Balance,
    UnitCost {
        #[arg(long, default_value = "일반")]
        receive_num_type: String,
    },
    ChargeInfo {
        #[arg(long, default_value = "일반")]
        receive_num_type: String,
    },
    Result {
        #[arg(long)]
        receipt_num: String,
    },
    CancelReserve {
        #[arg(long)]
        receipt_num: String,
        #[arg(long)]
        confirm_delete: bool,
    },
}

#[derive(Debug, Clone)]
struct Config {
    link_id: String,
    secret_key: String,
    corp_num: String,
    user_id: String,
    is_test: bool,
    ip_restrict: bool,
    use_static_ip: bool,
    use_local_time: bool,
    use_ga_ip: bool,
    approval_url: String,
    approval_timeout_secs: u64,
    siren_status_url: Option<String>,
    siren_wait_secs: u64,
}

#[derive(Debug, Default, Deserialize)]
struct TomlConfig {
    link_id: Option<String>,
    secret_key: Option<String>,
    corp_num: Option<String>,
    user_id: Option<String>,
    is_test: Option<bool>,
    ip_restrict: Option<bool>,
    use_static_ip: Option<bool>,
    use_local_time: Option<bool>,
    use_ga_ip: Option<bool>,
    approval_url: Option<String>,
    approval_timeout_secs: Option<u64>,
    siren_status_url: Option<String>,
    siren_wait_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FaxRequest {
    sender: String,
    #[serde(default)]
    sender_name: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "adsYN", default)]
    ads_yn: bool,
    #[serde(rename = "reserveDT", default)]
    reserve_dt: String,
    #[serde(default)]
    request_num: String,
    receivers: Vec<FaxReceiver>,
    files: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FaxReceiver {
    receive_num: String,
    #[serde(default)]
    receive_name: String,
    #[serde(rename = "interOPRefKey", default)]
    inter_op_ref_key: String,
}

#[derive(Debug, Deserialize)]
struct LinkhubToken {
    session_token: String,
    #[serde(rename = "serviceID")]
    service_id_upper: Option<String>,
    #[serde(rename = "serviceId")]
    service_id_lower: Option<String>,
}

#[derive(Debug, Serialize)]
struct PublicConfig {
    link_id: String,
    corp_num: String,
    user_id: String,
    is_test: bool,
}

#[derive(Debug, Deserialize)]
struct ApprovalCreateResponse {
    request_id: String,
}

#[derive(Debug, Deserialize)]
struct ApprovalPollResponse {
    decision: String,
    reason: Option<String>,
    elapsed_sec: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ApprovalPendingResponse {
    pending: Vec<ApprovalPendingItem>,
}

#[derive(Debug, Deserialize)]
struct ApprovalPendingItem {
    id: String,
    decided: bool,
}

struct FaxSearchQuery {
    start_date: Option<String>,
    end_date: Option<String>,
    state: Vec<String>,
    reserve_yn: Option<bool>,
    sender_only: Option<bool>,
    order: String,
    page: u32,
    per_page: u32,
    query: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::builder()
        .timeout(Duration::from_secs(180))
        .gzip(true)
        .deflate(true)
        .build()
        .context("failed to build HTTP client")?;

    match cli.command {
        Commands::DryRun { request } => {
            let request = read_fax_request(&request)?;
            print_json(json!({
                "ok": true,
                "mode": "dry-run",
                "sender": request.sender,
                "senderName": request.sender_name,
                "receiverCount": request.receivers.len(),
                "fileCount": request.files.len(),
                "files": request.files,
                "title": request.title,
                "reserveDT": request.reserve_dt,
                "adsYN": request.ads_yn
            }))?;
        }
        Commands::Send {
            request,
            confirm_send,
        } => {
            if !confirm_send {
                bail!("actual fax send is blocked unless --confirm-send is present");
            }
            let config = Config::load(&cli.config, &cli.env)?;
            let request = read_fax_request(&request)?;
            require_send_gate(&config, &request)?;
            let token = generate_token(&client, &config)?;
            let response = send_fax(&client, &config, &token, &request)?;
            print_json(json!({
                "ok": true,
                "mode": "send",
                "response": response,
                "config": config.public()
            }))?;
        }
        Commands::List {
            start_date,
            end_date,
            state,
            reserve_yn,
            sender_only,
            order,
            page,
            per_page,
            query,
        } => {
            let config = Config::load(&cli.config, &cli.env)?;
            let token = generate_token(&client, &config)?;
            let response = fax_search(
                &client,
                &config,
                &token,
                FaxSearchQuery {
                    start_date,
                    end_date,
                    state,
                    reserve_yn,
                    sender_only,
                    order,
                    page,
                    per_page,
                    query,
                },
            )?;
            print_json(
                json!({"ok": true, "mode": "list", "response": response, "config": config.public()}),
            )?;
        }
        Commands::Read {
            receipt_num,
            request_num,
        } => {
            let config = Config::load(&cli.config, &cli.env)?;
            let token = generate_token(&client, &config)?;
            let (mode, response) = read_fax(&client, &config, &token, receipt_num, request_num)?;
            print_json(
                json!({"ok": true, "mode": mode, "response": response, "config": config.public()}),
            )?;
        }
        Commands::Delete {
            receipt_num,
            request_num,
            confirm_delete,
        } => {
            if !confirm_delete {
                bail!(
                    "fax reservation cancel/delete is blocked unless --confirm-delete is present"
                );
            }
            let config = Config::load(&cli.config, &cli.env)?;
            require_cancel_gate(&config, receipt_num.as_deref(), request_num.as_deref())?;
            let token = generate_token(&client, &config)?;
            let (mode, response) = cancel_fax(&client, &config, &token, receipt_num, request_num)?;
            print_json(
                json!({"ok": true, "mode": mode, "response": response, "config": config.public()}),
            )?;
        }
        Commands::SenderList => {
            let config = Config::load(&cli.config, &cli.env)?;
            let token = generate_token(&client, &config)?;
            let response = popbill_get(&client, &config, &token, "/FAX/SenderNumber")?;
            print_json(
                json!({"ok": true, "mode": "sender-list", "response": response, "config": config.public()}),
            )?;
        }
        Commands::CheckSender { sender } => {
            let config = Config::load(&cli.config, &cli.env)?;
            let token = generate_token(&client, &config)?;
            let path = format!("/FAX/CheckSenderNumber/{sender}");
            let response = popbill_get(&client, &config, &token, &path)?;
            print_json(
                json!({"ok": true, "mode": "check-sender", "sender": sender, "response": response, "config": config.public()}),
            )?;
        }
        Commands::Balance => {
            let config = Config::load(&cli.config, &cli.env)?;
            let token = generate_token(&client, &config)?;
            let response = linkhub_get(&client, &config, &token, "Point")?;
            print_json(
                json!({"ok": true, "mode": "balance", "response": response, "config": config.public()}),
            )?;
        }
        Commands::UnitCost { receive_num_type } => {
            let config = Config::load(&cli.config, &cli.env)?;
            let token = generate_token(&client, &config)?;
            let mut url = popbill_url(&config, "/FAX/UnitCost")?;
            url.query_pairs_mut()
                .append_pair("receiveNumType", &receive_num_type);
            let response = popbill_get_url(&client, &config, &token, url)?;
            print_json(
                json!({"ok": true, "mode": "unit-cost", "receiveNumType": receive_num_type, "response": response, "config": config.public()}),
            )?;
        }
        Commands::ChargeInfo { receive_num_type } => {
            let config = Config::load(&cli.config, &cli.env)?;
            let token = generate_token(&client, &config)?;
            let mut url = popbill_url(&config, "/FAX/ChargeInfo")?;
            url.query_pairs_mut()
                .append_pair("receiveNumType", &receive_num_type);
            let response = popbill_get_url(&client, &config, &token, url)?;
            print_json(
                json!({"ok": true, "mode": "charge-info", "receiveNumType": receive_num_type, "response": response, "config": config.public()}),
            )?;
        }
        Commands::Result { receipt_num } => {
            let config = Config::load(&cli.config, &cli.env)?;
            let token = generate_token(&client, &config)?;
            let path = format!("/FAX/{receipt_num}");
            let response = popbill_get(&client, &config, &token, &path)?;
            print_json(
                json!({"ok": true, "mode": "result", "receiptNum": receipt_num, "response": response, "config": config.public()}),
            )?;
        }
        Commands::CancelReserve {
            receipt_num,
            confirm_delete,
        } => {
            if !confirm_delete {
                bail!(
                    "fax reservation cancel/delete is blocked unless --confirm-delete is present"
                );
            }
            let config = Config::load(&cli.config, &cli.env)?;
            require_cancel_gate(&config, Some(&receipt_num), None)?;
            let token = generate_token(&client, &config)?;
            let path = format!("/FAX/{receipt_num}/Cancel");
            let response = popbill_get(&client, &config, &token, &path)?;
            print_json(
                json!({"ok": true, "mode": "cancel-reserve", "receiptNum": receipt_num, "response": response, "config": config.public()}),
            )?;
        }
    }

    Ok(())
}

impl Config {
    fn load(config_path: &Path, env_path: &Path) -> Result<Self> {
        let mut values = read_toml_config(config_path)?;
        for (key, value) in read_env_file(env_path)? {
            values.insert(key, value);
        }
        for (key, value) in env::vars() {
            if key.starts_with("POPBILL_") {
                values.insert(key, value);
            }
        }

        let link_id = required(&values, "POPBILL_LINK_ID")?;
        let secret_key = required(&values, "POPBILL_SECRET_KEY")?;
        let corp_num = required(&values, "POPBILL_CORP_NUM")?;
        let user_id = values
            .get("POPBILL_USER_ID")
            .cloned()
            .unwrap_or_else(|| "hostingglobal".to_string());

        Ok(Self {
            link_id,
            secret_key,
            corp_num,
            user_id,
            is_test: bool_value(&values, "POPBILL_IS_TEST", false),
            ip_restrict: bool_value(&values, "POPBILL_IP_RESTRICT", true),
            use_static_ip: bool_value(&values, "POPBILL_USE_STATIC_IP", false),
            use_local_time: bool_value(&values, "POPBILL_USE_LOCAL_TIME", true),
            use_ga_ip: bool_value(&values, "POPBILL_USE_GA_IP", false),
            approval_url: string_value(
                &values,
                "POPBILL_FAX_APPROVAL_URL",
                "http://127.0.0.1:5510",
            ),
            approval_timeout_secs: u64_value(&values, "POPBILL_FAX_APPROVAL_TIMEOUT_SEC", 600),
            siren_status_url: optional_value(&values, "POPBILL_FAX_SIREN_STATUS_URL")
                .or_else(|| optional_value(&values, "WARNING_LIGHT_STATUS_URL"))
                .or_else(|| optional_value(&values, "SIREN_STATUS_URL")),
            siren_wait_secs: u64_value(&values, "POPBILL_FAX_SIREN_WAIT_SEC", 12),
        })
    }

    fn service_id(&self) -> &'static str {
        if self.is_test {
            SERVICE_ID_TEST
        } else {
            SERVICE_ID_REAL
        }
    }

    fn public(&self) -> PublicConfig {
        PublicConfig {
            link_id: mask(&self.link_id),
            corp_num: mask(&self.corp_num),
            user_id: self.user_id.clone(),
            is_test: self.is_test,
        }
    }
}

fn read_toml_config(path: &Path) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    let path = if path.exists() {
        path.to_path_buf()
    } else if path == Path::new("config.toml") {
        let Some(home) = env::var_os("HOME") else {
            return Ok(values);
        };
        let fallback = PathBuf::from(home).join(".config/popbill-fax/config.toml");
        if fallback.exists() {
            fallback
        } else {
            return Ok(values);
        }
    } else {
        return Ok(values);
    };

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let parsed: TomlConfig = toml::from_str(&content)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;

    insert_optional(&mut values, "POPBILL_LINK_ID", parsed.link_id);
    insert_optional(&mut values, "POPBILL_SECRET_KEY", parsed.secret_key);
    insert_optional(&mut values, "POPBILL_CORP_NUM", parsed.corp_num);
    insert_optional(&mut values, "POPBILL_USER_ID", parsed.user_id);
    insert_optional_bool(&mut values, "POPBILL_IS_TEST", parsed.is_test);
    insert_optional_bool(&mut values, "POPBILL_IP_RESTRICT", parsed.ip_restrict);
    insert_optional_bool(&mut values, "POPBILL_USE_STATIC_IP", parsed.use_static_ip);
    insert_optional_bool(&mut values, "POPBILL_USE_LOCAL_TIME", parsed.use_local_time);
    insert_optional_bool(&mut values, "POPBILL_USE_GA_IP", parsed.use_ga_ip);
    insert_optional(&mut values, "POPBILL_FAX_APPROVAL_URL", parsed.approval_url);
    if let Some(value) = parsed.approval_timeout_secs {
        values.insert(
            "POPBILL_FAX_APPROVAL_TIMEOUT_SEC".to_string(),
            value.to_string(),
        );
    }
    insert_optional(
        &mut values,
        "POPBILL_FAX_SIREN_STATUS_URL",
        parsed.siren_status_url,
    );
    if let Some(value) = parsed.siren_wait_secs {
        values.insert("POPBILL_FAX_SIREN_WAIT_SEC".to_string(), value.to_string());
    }

    Ok(values)
}

fn insert_optional(values: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        values.insert(key.to_string(), value);
    }
}

fn insert_optional_bool(values: &mut BTreeMap<String, String>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        values.insert(key.to_string(), value.to_string());
    }
}

fn read_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    if !path.exists() {
        return Ok(values);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read env file {}", path.display()))?;
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let Some((key, raw_value)) = trimmed.split_once('=') else {
            bail!("invalid env line {} in {}", line_no + 1, path.display());
        };
        let key = key.trim();
        if key.is_empty() {
            bail!(
                "empty env key on line {} in {}",
                line_no + 1,
                path.display()
            );
        }
        values.insert(key.to_string(), strip_quotes(raw_value.trim()).to_string());
    }
    Ok(values)
}

fn strip_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let first = value.as_bytes()[0];
        let last = value.as_bytes()[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn required(values: &BTreeMap<String, String>, key: &str) -> Result<String> {
    let value = values
        .get(key)
        .with_context(|| format!("{key} is required"))?
        .trim()
        .to_string();
    if value.is_empty() {
        bail!("{key} is empty");
    }
    Ok(value)
}

fn optional_value(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values.get(key).and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

fn string_value(values: &BTreeMap<String, String>, key: &str, default_value: &str) -> String {
    optional_value(values, key).unwrap_or_else(|| default_value.to_string())
}

fn bool_value(values: &BTreeMap<String, String>, key: &str, default_value: bool) -> bool {
    values
        .get(key)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "y" | "on"
            )
        })
        .unwrap_or(default_value)
}

fn u64_value(values: &BTreeMap<String, String>, key: &str, default_value: u64) -> u64 {
    values
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default_value)
}

fn read_fax_request(path: &Path) -> Result<FaxRequest> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read request JSON {}", path.display()))?;
    let request: FaxRequest = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse request JSON {}", path.display()))?;
    validate_fax_request(&request)?;
    Ok(request)
}

fn validate_fax_request(request: &FaxRequest) -> Result<()> {
    if request.sender.trim().is_empty() {
        bail!("sender is required");
    }
    if request.receivers.is_empty() {
        bail!("receivers must not be empty");
    }
    if request.receivers.len() > 1000 {
        bail!("receivers max is 1000");
    }
    for (idx, receiver) in request.receivers.iter().enumerate() {
        if receiver.receive_num.trim().is_empty() {
            bail!("receivers[{idx}].receiveNum is required");
        }
    }
    if request.files.is_empty() {
        bail!("files must not be empty");
    }
    if request.files.len() > 20 {
        bail!("files max is 20");
    }
    for file in &request.files {
        if !file.exists() {
            bail!("file not found: {}", file.display());
        }
        if !file.is_file() {
            bail!("not a regular file: {}", file.display());
        }
    }
    Ok(())
}

fn require_send_gate(config: &Config, request: &FaxRequest) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(
            config.approval_timeout_secs.saturating_add(15),
        ))
        .gzip(true)
        .deflate(true)
        .build()
        .context("failed to build approval HTTP client")?;
    let created = create_approval_request(&client, config, request)?;
    verify_siren_signal(&client, config, &created.request_id)?;
    require_telegram_approval(&client, config, &created.request_id)?;
    Ok(())
}

fn require_cancel_gate(
    config: &Config,
    receipt_num: Option<&str>,
    request_num: Option<&str>,
) -> Result<()> {
    validate_one_lookup_key(receipt_num, request_num)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(
            config.approval_timeout_secs.saturating_add(15),
        ))
        .gzip(true)
        .deflate(true)
        .build()
        .context("failed to build approval HTTP client")?;
    let create_url = approval_url(config, "/defer")?;
    let payload = json!({
        "summary": cancel_approval_summary(receipt_num, request_num),
        "tool": "popbill-fax-delete",
        "node": env::var("CCC_NODE").unwrap_or_else(|_| "wsl".to_string()),
    });
    let created: ApprovalCreateResponse = json_response(
        client
            .post(create_url)
            .header(CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .context("failed to create Telegram approval request")?,
    )
    .context("Telegram approval request failed")?;
    verify_siren_signal(&client, config, &created.request_id)?;
    require_telegram_approval(&client, config, &created.request_id)?;
    Ok(())
}

fn create_approval_request(
    client: &Client,
    config: &Config,
    request: &FaxRequest,
) -> Result<ApprovalCreateResponse> {
    let create_url = approval_url(config, "/defer")?;
    let payload = json!({
        "summary": approval_summary(request),
        "tool": "popbill-fax",
        "node": env::var("CCC_NODE").unwrap_or_else(|_| "wsl".to_string()),
    });
    json_response(
        client
            .post(create_url)
            .header(CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .context("failed to create Telegram approval request")?,
    )
    .context("Telegram approval request failed")
}

fn require_telegram_approval(client: &Client, config: &Config, request_id: &str) -> Result<()> {
    let poll_path = format!("/poll/{request_id}");
    let decision: ApprovalPollResponse = json_response(
        client
            .get(approval_url(config, &poll_path)?)
            .send()
            .context("failed to poll Telegram approval request")?,
    )
    .context("Telegram approval poll failed")?;

    if decision.decision != "allow" {
        bail!(
            "fax send denied by Telegram approval gate: decision={} reason={} elapsed_sec={}",
            decision.decision,
            decision.reason.unwrap_or_else(|| "none".to_string()),
            decision.elapsed_sec.unwrap_or_default()
        );
    }

    Ok(())
}

fn verify_siren_signal(client: &Client, config: &Config, request_id: &str) -> Result<()> {
    verify_pending_request_visible(client, config, request_id)?;
    if let Some(status_url) = &config.siren_status_url {
        verify_siren_status(client, status_url, config.siren_wait_secs)?;
    }
    Ok(())
}

fn verify_pending_request_visible(
    client: &Client,
    config: &Config,
    request_id: &str,
) -> Result<()> {
    let pending: ApprovalPendingResponse =
        json_response(client.get(approval_url(config, "/pending")?).send()?)
            .context("failed to read approval pending list")?;
    let visible = pending
        .pending
        .iter()
        .any(|item| item.id == request_id && !item.decided);
    if !visible {
        bail!(
            "fax send blocked: approval request is not visible as pending, so siren cannot poll it"
        );
    }
    Ok(())
}

fn verify_siren_status(client: &Client, status_url: &str, wait_secs: u64) -> Result<()> {
    let status_url =
        Url::parse(status_url).with_context(|| format!("invalid siren status URL {status_url}"))?;
    let attempts = wait_secs.max(1);
    for _ in 0..attempts {
        let response = client
            .get(status_url.clone())
            .send()
            .with_context(|| format!("failed to call siren status URL {status_url}"))?;
        if response.status().is_success() {
            let body = response.text().unwrap_or_default();
            if siren_body_indicates_active(&body) {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    bail!(
        "fax send blocked: siren status URL did not report active approval/siren within {attempts}s"
    );
}

fn siren_body_indicates_active(body: &str) -> bool {
    let compact = body
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    compact.contains("\"approvalPending\":true")
        || compact.contains("\"siren\":true")
        || compact.contains("\"sirenActive\":true")
        || compact.contains("\"approvalCount\":1")
        || compact.contains("\"approvalPendingCount\":1")
}

fn approval_url(config: &Config, path: &str) -> Result<Url> {
    let base = config.approval_url.trim().trim_end_matches('/');
    Url::parse(&format!("{base}{path}")).context("failed to build approval URL")
}

fn approval_summary(request: &FaxRequest) -> String {
    let title = if request.title.trim().is_empty() {
        "(제목 없음)"
    } else {
        request.title.trim()
    };
    let request_num = if request.request_num.trim().is_empty() {
        "(없음)"
    } else {
        request.request_num.trim()
    };
    let file_names = request
        .files
        .iter()
        .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
        .take(5)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Popbill 팩스 실제 발송 승인 요청\n발신: {}\n수신: {}건\n파일: {}개 [{}]\n제목: {}\n요청번호: {}",
        mask(&request.sender),
        request.receivers.len(),
        request.files.len(),
        file_names,
        title,
        request_num
    )
}

fn cancel_approval_summary(receipt_num: Option<&str>, request_num: Option<&str>) -> String {
    let target = receipt_num
        .map(|value| format!("접수번호: {value}"))
        .or_else(|| request_num.map(|value| format!("요청번호: {value}")))
        .unwrap_or_else(|| "(대상 없음)".to_string());
    format!("Popbill 팩스 예약 취소/삭제 승인 요청\n{target}")
}

fn generate_token(client: &Client, config: &Config) -> Result<LinkhubToken> {
    let service_id = config.service_id();
    let body = serde_json::to_string(&json!({
        "access_id": config.corp_num,
        "scope": ["member", "160", "161"]
    }))?;
    let call_dt = linkhub_time(client, config)?;
    let uri = format!("/{service_id}/Token");
    let forward_ip = if config.ip_restrict { None } else { Some("*") };

    let mut hmac_target = String::new();
    hmac_target.push_str("POST\n");
    hmac_target.push_str(&b64_sha256(&body));
    hmac_target.push('\n');
    hmac_target.push_str(&call_dt);
    hmac_target.push('\n');
    if let Some(value) = forward_ip {
        hmac_target.push_str(value);
        hmac_target.push('\n');
    }
    hmac_target.push_str(LINKHUB_VERSION);
    hmac_target.push('\n');
    hmac_target.push_str(&uri);

    let signature = b64_hmac_sha256(&config.secret_key, &hmac_target)?;
    let mut headers = HeaderMap::new();
    headers.insert("x-lh-date", HeaderValue::from_str(&call_dt)?);
    headers.insert("x-lh-version", HeaderValue::from_static(LINKHUB_VERSION));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("Application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static("RUST LINKHUB SDK"));
    if let Some(value) = forward_ip {
        headers.insert("x-lh-forwarded", HeaderValue::from_static(value));
    }
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("LINKHUB {} {}", config.link_id, signature))?,
    );

    let url = format!("{}{}", linkhub_base(config), uri);
    let response = client.post(url).headers(headers).body(body).send()?;
    json_response(response).context("token request failed")
}

fn fax_search(
    client: &Client,
    config: &Config,
    token: &LinkhubToken,
    query: FaxSearchQuery,
) -> Result<Value> {
    let start_date = query.start_date.unwrap_or_else(today_yyyymmdd);
    let end_date = query.end_date.unwrap_or_else(today_yyyymmdd);
    validate_yyyymmdd("start-date", &start_date)?;
    validate_yyyymmdd("end-date", &end_date)?;
    if query.per_page == 0 || query.per_page > 1000 {
        bail!("per-page must be between 1 and 1000");
    }
    if query.page == 0 {
        bail!("page must be >= 1");
    }

    let mut url = popbill_url(config, "/FAX/Search")?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("SDate", &start_date);
        pairs.append_pair("EDate", &end_date);
        if !query.state.is_empty() {
            pairs.append_pair("State", &query.state.join(","));
        }
        if let Some(value) = query.reserve_yn {
            pairs.append_pair("ReserveYN", if value { "1" } else { "0" });
        }
        if let Some(value) = query.sender_only {
            pairs.append_pair("SenderOnly", if value { "1" } else { "0" });
        }
        if !query.order.trim().is_empty() {
            pairs.append_pair("Order", query.order.trim());
        }
        pairs.append_pair("Page", &query.page.to_string());
        pairs.append_pair("PerPage", &query.per_page.to_string());
        if let Some(value) = query
            .query
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            pairs.append_pair("QString", value.trim());
        }
    }
    popbill_get_url(client, config, token, url)
}

fn read_fax(
    client: &Client,
    config: &Config,
    token: &LinkhubToken,
    receipt_num: Option<String>,
    request_num: Option<String>,
) -> Result<(&'static str, Value)> {
    validate_one_lookup_key(receipt_num.as_deref(), request_num.as_deref())?;
    if let Some(value) = receipt_num {
        let path = format!("/FAX/{value}");
        Ok(("read", popbill_get(client, config, token, &path)?))
    } else {
        let value = request_num.expect("validated request_num");
        let path = format!("/FAX/Get/{value}");
        Ok((
            "read-request-num",
            popbill_get(client, config, token, &path)?,
        ))
    }
}

fn cancel_fax(
    client: &Client,
    config: &Config,
    token: &LinkhubToken,
    receipt_num: Option<String>,
    request_num: Option<String>,
) -> Result<(&'static str, Value)> {
    validate_one_lookup_key(receipt_num.as_deref(), request_num.as_deref())?;
    if let Some(value) = receipt_num {
        let path = format!("/FAX/{value}/Cancel");
        Ok((
            "delete-cancel-reserve",
            popbill_get(client, config, token, &path)?,
        ))
    } else {
        let value = request_num.expect("validated request_num");
        let path = format!("/FAX/Cancel/{value}");
        Ok((
            "delete-cancel-reserve-request-num",
            popbill_get(client, config, token, &path)?,
        ))
    }
}

fn validate_one_lookup_key(receipt_num: Option<&str>, request_num: Option<&str>) -> Result<()> {
    let has_receipt = receipt_num
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let has_request = request_num
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    match (has_receipt, has_request) {
        (true, false) | (false, true) => Ok(()),
        (false, false) => bail!("one of --receipt-num or --request-num is required"),
        (true, true) => bail!("use only one of --receipt-num or --request-num"),
    }
}

fn today_yyyymmdd() -> String {
    Utc::now().format("%Y%m%d").to_string()
}

fn validate_yyyymmdd(label: &str, value: &str) -> Result<()> {
    if value.len() == 8 && value.chars().all(|c| c.is_ascii_digit()) {
        Ok(())
    } else {
        bail!("{label} must use YYYYMMDD format")
    }
}

fn linkhub_time(client: &Client, config: &Config) -> Result<String> {
    if config.use_local_time {
        return Ok(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
    }
    let response = client
        .get(format!("{}/Time", linkhub_base(config)))
        .header(USER_AGENT, "RUST LINKHUB SDK")
        .send()?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().unwrap_or_default();
        bail!("Linkhub /Time failed: {status} {text}");
    }
    Ok(response.text()?.trim_matches('"').to_string())
}

fn send_fax(
    client: &Client,
    config: &Config,
    token: &LinkhubToken,
    request: &FaxRequest,
) -> Result<Value> {
    let mut form_json = Map::new();
    form_json.insert("snd".to_string(), json!(request.sender));
    if !request.sender_name.trim().is_empty() {
        form_json.insert("sndnm".to_string(), json!(request.sender_name));
    }
    form_json.insert("fCnt".to_string(), json!(request.files.len()));

    let receivers = request
        .receivers
        .iter()
        .map(|receiver| {
            let mut item = Map::new();
            item.insert("rcv".to_string(), json!(receiver.receive_num));
            if !receiver.receive_name.trim().is_empty() {
                item.insert("rcvnm".to_string(), json!(receiver.receive_name));
            }
            if !receiver.inter_op_ref_key.trim().is_empty() {
                item.insert(
                    "interOPRefKey".to_string(),
                    json!(receiver.inter_op_ref_key),
                );
            }
            Value::Object(item)
        })
        .collect::<Vec<_>>();
    form_json.insert("rcvs".to_string(), Value::Array(receivers));

    if !request.reserve_dt.trim().is_empty() {
        form_json.insert("sndDT".to_string(), json!(request.reserve_dt));
    }
    if request.ads_yn {
        form_json.insert("adsYN".to_string(), json!(true));
    }
    if !request.title.trim().is_empty() {
        form_json.insert("title".to_string(), json!(request.title));
    }
    if !request.request_num.trim().is_empty() {
        form_json.insert("requestNum".to_string(), json!(request.request_num));
    }

    let mut form = multipart::Form::new().text("form", Value::Object(form_json).to_string());
    for file in &request.files {
        let file_name = file
            .file_name()
            .and_then(|name| name.to_str())
            .context("file name is not valid UTF-8")?
            .to_string();
        let data = fs::read(file).with_context(|| format!("failed to read {}", file.display()))?;
        let part = multipart::Part::bytes(data)
            .file_name(file_name)
            .mime_str("Application/octet-stream")?;
        form = form.part("file", part);
    }

    let mut headers = popbill_headers(config, token)?;
    headers.remove(CONTENT_TYPE);
    let url = popbill_url(config, "/FAX")?;
    let response = client.post(url).headers(headers).multipart(form).send()?;
    json_response(response).context("fax send failed")
}

fn popbill_get(
    client: &Client,
    config: &Config,
    token: &LinkhubToken,
    path: &str,
) -> Result<Value> {
    let url = popbill_url(config, path)?;
    popbill_get_url(client, config, token, url)
}

fn popbill_get_url(
    client: &Client,
    config: &Config,
    token: &LinkhubToken,
    url: Url,
) -> Result<Value> {
    let response = client
        .get(url)
        .headers(popbill_headers(config, token)?)
        .send()?;
    json_response(response)
}

fn linkhub_get(
    client: &Client,
    config: &Config,
    token: &LinkhubToken,
    endpoint: &str,
) -> Result<Value> {
    let service_id = token
        .service_id_upper
        .as_deref()
        .or(token.service_id_lower.as_deref())
        .unwrap_or(config.service_id());
    let url = format!("{}/{service_id}/{endpoint}", linkhub_base(config));
    let response = client
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {}", token.session_token))
        .header(USER_AGENT, "RUST LINKHUB SDK")
        .send()?;
    json_response(response)
}

fn popbill_headers(config: &Config, token: &LinkhubToken) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert("x-pb-version", HeaderValue::from_static(POPBILL_VERSION));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token.session_token))?,
    );
    headers.insert("x-pb-userid", HeaderValue::from_str(&config.user_id)?);
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip,deflate"));
    headers.insert(USER_AGENT, HeaderValue::from_static("RUST POPBILL SDK"));
    Ok(headers)
}

fn json_response<T: for<'de> Deserialize<'de>>(response: reqwest::blocking::Response) -> Result<T> {
    let status = response.status();
    let text = response.text()?;
    if !status.is_success() {
        let parsed =
            serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "raw": text }));
        bail!("HTTP {status}: {}", serde_json::to_string(&parsed)?);
    }
    serde_json::from_str(&text).with_context(|| format!("failed to parse JSON response: {text}"))
}

fn linkhub_base(config: &Config) -> &'static str {
    if config.use_ga_ip {
        "https://ga-auth.linkhub.co.kr"
    } else if config.use_static_ip {
        "https://static-auth.linkhub.co.kr"
    } else {
        "https://auth.linkhub.co.kr"
    }
}

fn popbill_base(config: &Config) -> &'static str {
    match (config.is_test, config.use_ga_ip, config.use_static_ip) {
        (true, true, _) => "https://ga-popbill-test.linkhub.co.kr",
        (false, true, _) => "https://ga-popbill.linkhub.co.kr",
        (true, _, true) => "https://static-popbill-test.linkhub.co.kr",
        (false, _, true) => "https://static-popbill.linkhub.co.kr",
        (true, _, _) => "https://popbill-test.linkhub.co.kr",
        (false, _, _) => "https://popbill.linkhub.co.kr",
    }
}

fn popbill_url(config: &Config, path: &str) -> Result<Url> {
    let base = popbill_base(config);
    Url::parse(&format!("{base}{path}")).context("failed to build Popbill URL")
}

fn b64_sha256(input: &str) -> String {
    B64.encode(Sha256::digest(input.as_bytes()))
}

fn b64_hmac_sha256(secret_key: &str, target: &str) -> Result<String> {
    let key = B64
        .decode(secret_key.as_bytes())
        .context("POPBILL_SECRET_KEY must be base64")?;
    let mut mac = HmacSha256::new_from_slice(&key).context("failed to initialize HMAC")?;
    mac.update(target.as_bytes());
    Ok(B64.encode(mac.finalize().into_bytes()))
}

fn mask(value: &str) -> String {
    if value.len() <= 8 {
        "********".to_string()
    } else {
        format!("{}...{}", &value[..4], &value[value.len() - 4..])
    }
}

fn print_json(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
