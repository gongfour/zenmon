# zenmon-cli Design Spec

## Overview

Zenoh 네트워크 모니터링 및 개발/디버깅을 위한 Rust CLI + TUI 도구.
기존 HDX 웹 도구(hdx_monitor, hdx_monitor_zenoh)의 한계를 극복하고, 터미널에서 가볍고 빠르게 Zenoh 네트워크를 탐색/모니터링한다.

### 핵심 동기

- 브라우저를 계속 띄워두지 않고 터미널에서 바로 접근
- REST API의 한계 (attachment 등 미지원) 대신 Zenoh native API 직접 사용
- 스크립팅/파이프라인 친화적인 headless CLI + 인터랙티브 TUI 병행

## Architecture

### 프로젝트 구조 (Cargo workspace)

```
zenmon_cli/
├── Cargo.toml              # workspace root
├── crates/
│   ├── zenmon-core/        # Zenoh 연결, 디스커버리, 구독, 쿼리 로직
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── session.rs      # Zenoh 세션 생성/관리
│   │       ├── discover.rs     # 토픽 디스커버리 (admin space 쿼리)
│   │       ├── subscriber.rs   # 토픽 구독, 메시지 스트림
│   │       ├── query.rs        # Zenoh GET 요청/응답
│   │       ├── registry.rs     # 노드 레지스트리 조회
│   │       ├── config.rs       # 연결 설정
│   │       └── types.rs        # 공통 타입 (TopicInfo, Message, NodeInfo 등)
│   ├── zenmon-cli/         # clap 기반 서브커맨드 (headless CLI)
│   │   ├── Cargo.toml
│   │   └── src/main.rs
│   └── zenmon-tui/         # ratatui TUI 대시보드
│       ├── Cargo.toml
│       └── src/main.rs
```

3개 크레이트:

- **zenmon-core** — Zenoh 세션 관리, 토픽 디스커버리, 구독 스트림, 쿼리, 노드 레지스트리 조회. CLI와 TUI 모두 이 라이브러리에 의존.
- **zenmon-cli** — `clap` 서브커맨드. stdout으로 JSON/텍스트 출력. 스크립팅/파이프라인 친화적. `zenmon tui` 서브커맨드로 TUI 실행. zenmon-tui를 라이브러리로 의존.
- **zenmon-tui** — ratatui 기반 인터랙티브 TUI. 라이브러리 크레이트 (별도 바이너리 아님). zenmon-cli에서 호출.

바이너리는 `zenmon` 하나 (zenmon-cli가 생성). `zenmon tui`로 TUI 모드 진입.

## CLI 서브커맨드

```
zenmon <COMMAND> [OPTIONS]

Global Options:
  -e, --endpoint <ENDPOINT>    Zenoh 연결 엔드포인트 (기본: tcp/localhost:7447)
  -m, --mode <MODE>            peer | client (기본: client)
  -n, --namespace <NS>         Zenoh namespace (네이티브 프리픽스 격리)
  -c, --config <FILE>          Zenoh json5 설정 파일 경로
  --json                       JSON 출력

Commands:
  discover    활성 토픽/키 탐색
  sub         토픽 구독 (실시간 메시지 스트림)
  query       Zenoh queryable에 GET 요청
  nodes       노드 레지스트리 조회
  tui         인터랙티브 TUI 대시보드 실행
```

### 서브커맨드 상세

```
zenmon discover [KEY_EXPR]
  # 활성 키 목록 출력. KEY_EXPR 생략 시 "**"
  # 예: zenmon discover "forklift/**"

zenmon sub <KEY_EXPR> [--pretty] [--timestamp]
  # 실시간 메시지 스트림 출력. Ctrl+C로 종료.
  # 예: zenmon sub "forklift/snapshot" --pretty

zenmon query <KEY_EXPR> [--payload <JSON>] [--timeout <MS>]
  # Zenoh GET 요청 후 응답 출력
  # 예: zenmon query "forklift/status"

zenmon nodes [--watch]
  # 등록된 노드 목록 출력
  # --watch: 변경 시 실시간 업데이트

zenmon tui [--refresh <MS>]
  # TUI 대시보드 실행 (기본 refresh: 100ms)
```

출력은 기본 텍스트, `--json` 플래그 시 JSON 형식으로 전환.

## Zenoh 연결

- **클라이언트 모드:** zenohd 라우터에 연결 (`--endpoint`)
- **피어 모드:** P2P 직접 통신 (`--mode peer`)
- `--namespace`: Zenoh 네이티브 namespace 설정에 매핑 (키 프리픽스 자동 격리)
- `--config`: Zenoh json5 설정 파일을 직접 전달하여 멀티캐스트, scouting 등 세부 설정 가능

JSON 데이터를 우선 지원. 수신 페이로드를 `serde_json::Value`로 파싱 시도, 실패 시 raw bytes 길이만 표시. 바이너리 디코딩은 향후 확장.

## TUI 뷰 구성

키보드 숫자키로 단일 뷰 전환:

```
[1] Dashboard  [2] Topics  [3] Subscribe  [4] Query  [5] Nodes
```

### 1) Dashboard 뷰 (기본)

- 연결 상태 (endpoint, mode, namespace)
- 활성 토픽 수, 노드 수 요약
- 최근 메시지 활동 피드 (최근 N개 토픽의 마지막 메시지 타임스탬프)
- 노드 heartbeat 상태

### 2) Topics 뷰

- 활성 토픽 목록 (key expression)
- 필터링/검색 (`/`로 입력, 예: `forklift/**`)
- 토픽 선택 후 Enter → Subscribe 뷰로 전환

### 3) Subscribe 뷰

- 선택한 토픽의 실시간 메시지 스트림
- JSON pretty-print
- 일시정지/재개 (스크롤백)
- 여러 토픽 동시 구독 가능

### 4) Query 뷰

- key expression 입력 → Zenoh GET 실행
- 응답 목록 표시 (JSON pretty-print)
- 쿼리 히스토리 (이전 쿼리 재실행)

### 5) Nodes 뷰

- Zenoh admin space (`@/session/**`, `@/router/**`) 쿼리를 통한 피어/세션 목록
- 노드 상태 (online/offline), 마지막 heartbeat 시간
- 노드 선택 → 해당 노드 관련 토픽 필터링

### 공통 UX

- `q` / `Esc` — 뒤로/종료
- `/` — 검색/필터 입력
- `?` — 키바인딩 도움말
- 하단 상태바 — 연결 상태, namespace, 현재 뷰 표시

## 비동기 아키텍처

```
┌─────────────┐     ┌──────────────────┐     ┌─────────────┐
│  CLI / TUI  │────>│   zenmon-core    │────>│    Zenoh     │
│  (frontend) │<────│                  │<────│   Session    │
└─────────────┘     └──────────────────┘     └─────────────┘
                         │
                    tokio runtime
                         │
              ┌──────────┼──────────┐
              v          v          v
         discover    subscriber   query
          task         task       task
```

- **CLI 모드:** 서브커맨드별로 tokio 태스크 하나 실행, 결과를 stdout 출력, 완료 시 종료
- **TUI 모드:** 이벤트 루프가 3가지 소스를 처리

```rust
enum AppEvent {
    Terminal(crossterm::event::Event),  // 키 입력
    Zenoh(ZenohMessage),               // 토픽 메시지
    Tick,                               // 주기적 UI 갱신
}

loop {
    let event = event_rx.recv().await;
    match event {
        AppEvent::Terminal(e) => handle_input(e),
        AppEvent::Zenoh(msg) => update_state(msg),
        AppEvent::Tick => render(&mut terminal),
    }
}
```

Zenoh 메시지는 `tokio::sync::mpsc` 채널로 TUI 이벤트 루프에 전달.

### 에러 처리

- Zenoh 연결 실패 시 재연결 시도 (exponential backoff)
- TUI 상태바에 연결 상태 표시 (Connected / Reconnecting / Disconnected)

## 설정

### 우선순위

```
CLI 플래그 > 환경변수 > 설정 파일 > 기본값
```

| 설정 | CLI 플래그 | 환경변수 | 기본값 |
|------|-----------|---------|--------|
| 엔드포인트 | `--endpoint` | `ZENMON_ENDPOINT` | `tcp/localhost:7447` |
| 모드 | `--mode` | `ZENMON_MODE` | `client` |
| namespace | `--namespace` | `ZENMON_NAMESPACE` | (없음) |
| 설정파일 | `--config` | `ZENMON_CONFIG` | (없음) |

### 주요 의존성

| 크레이트 | 용도 | 적용 범위 |
|---------|------|----------|
| `zenoh` | Zenoh Rust SDK | core |
| `tokio` | 비동기 런타임 | core, cli, tui |
| `serde` + `serde_json` | JSON 직렬화 | core |
| `clap` (derive) | CLI 파싱 | cli |
| `ratatui` + `crossterm` | TUI 렌더링 | tui |
| `tracing` | 로깅 | core, cli, tui |

### Rust Edition & MSRV

- **Rust Edition:** 2021
- **MSRV:** zenoh Rust SDK 최소 요구 버전에 맞춤 (1.75+)

## Scope

### MVP (v0.1)

- zenmon-core: 세션 관리, 디스커버리, 구독, 쿼리
- zenmon-cli: discover, sub, query, nodes 서브커맨드
- zenmon-tui: 5개 뷰 (Dashboard, Topics, Subscribe, Query, Nodes)

### 향후 확장 가능

- 퍼블리시 (publish) 커맨드
- 레코딩/리플레이
- 바이너리 페이로드 디코딩 (msgpack, protobuf 등)
- AGV 플릿 운영 기능 (미션 디스패치, 알람 관리)
