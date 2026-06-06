<p align="center">
  <img src="assets/hero.png" alt="popbill-fax" width="100%">
</p>

# popbill-fax

Popbill(LINKHUB) 팩스 API를 직접 호출하는 **순수 Rust 단일 CLI**. 목록조회·읽기·발송·예약취소. Node/Python 런타임 의존 없음 — 단일 정적 바이너리.

공식 Popbill / Linkhub SDK의 HMAC-SHA256 토큰 인증과 FAX 엔드포인트를 대조하여 구현한 비공식 공개판입니다. 수신 팩스함 기능이 아니라, 발송 내역 조회와 발송 자동화를 위한 CLI입니다.

## 기능

| 명령 | 설명 |
|------|------|
| `list` | 전송내역 목록조회 (`/FAX/Search`) |
| `read` | 전송결과 상세 조회 |
| `send` | 팩스 발송 (`--confirm-send` 필수, 과금) |
| `delete` | 예약(미발송) 팩스 취소 |
| `dry-run` | 발송 없이 요청 검증 |
| `balance` `unit-cost` `charge-info` | 포인트·단가·과금정보 |
| `sender-list` `check-sender` | 발신번호 관리 |

## 설치

```bash
cargo build --release
# 바이너리: target/release/popbill-fax
```

## 설정

```bash
cp env.example .env
# 또는
cp config.example.toml config.toml
```

`.env` 값:

```bash
POPBILL_LINK_ID=...
POPBILL_SECRET_KEY=...
POPBILL_CORP_NUM=1234567890
POPBILL_USER_ID=your-popbill-user-id
POPBILL_IS_TEST=false        # true=테스트(popbill-test), false=운영(popbill)
```

- 팝빌 팩스 API 인증 = `LinkID` / `SecretKey` / `CorpNum` / `UserID` (계정 로그인 비밀번호 아님).
- `POPBILL_USER_ID` 는 더미 기본값으로 대체하지 않고 반드시 설정해야 합니다.
- 설정 우선순위: `config.toml` → `.env` → 프로세스 환경변수 `POPBILL_*`.
- `.env` / `.env.*` / `config.toml` 은 git 제외(`.gitignore`). **인증키 커밋 금지.**

## 요청 JSON (send)

```json
{
  "sender": "0212345678",
  "senderName": "발신자명",
  "title": "Fax title",
  "adsYN": false,
  "reserveDT": "",
  "requestNum": "unique-request-number",
  "receivers": [
    { "receiveNum": "0211112222", "receiveName": "수신자", "interOPRefKey": "internal-key" }
  ],
  "files": [ "/absolute/path/to/document.pdf" ]
}
```

제한: 파일 최대 20개 · 수신자 최대 1000명 · `reserveDT` 빈 문자열=즉시, 예약=`yyyyMMddHHmmss`. 실제 발송 첨부는 Popbill이 변환 가능한 PDF/JPG/TIFF/HWP/DOC 계열 문서를 사용하세요.

## 사용

```bash
# 발송 없이 검증
popbill-fax dry-run --request samples/fax_request.example.json

# 목록조회 (기본 최근 내역)
popbill-fax list --start-date 20260601 --end-date 20260606 --page 1 --per-page 20

# 읽기 (전송결과)
popbill-fax read --receipt-num <receiptNum>

# 발송 (과금 — --confirm-send 필수)
popbill-fax send --request /path/to/request.json --confirm-send

# 삭제/취소 (예약 발송 취소 — --confirm-delete 필수)
popbill-fax delete --receipt-num <receiptNum> --confirm-delete

# 보조
popbill-fax balance
popbill-fax unit-cost --receive-num-type 일반
popbill-fax sender-list
```

## 안전장치

- 기본 명령은 `dry-run`. 실제 전송은 `send --confirm-send` 가 있어야만 진행.
- 삭제/취소는 `delete --confirm-delete` 가 있어야만 진행.
- 발송과 예약취소는 `POPBILL_FAX_APPROVAL_URL` 승인 게이트가 허용해야 실제 API를 호출합니다. 기본값은 로컬 개발용 `http://127.0.0.1:5510` 입니다.
- 출력은 `LinkID` 와 사업자번호를 마스킹하고 `SecretKey` 는 출력하지 않음.

## 라이선스

© 호스팅글로벌(주). 내부 도구 공개판.
