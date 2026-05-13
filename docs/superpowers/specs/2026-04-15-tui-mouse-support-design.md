# zemon TUI 마우스 지원 설계

## 목표

zemon TUI에 마우스 기반 탐색과 전용 키 기반 클립보드 복사를 추가한다. 마우스는 키보드를 모르는 사용자도 핵심 동작을 수행할 수 있도록 보조하고, 복사는 별도 키(`y` yank)로 분리해 마우스 캡처와 텍스트 선택이 충돌하지 않게 한다.

## 배경

현재 TUI는 `ratatui::init()`으로 터미널을 초기화하며 마우스 캡처를 켜지 않는다. `event.rs`는 `Event::Key`만 처리하고 `Event::Mouse`는 무시한다. 모든 인터랙션(탭 전환, 리스트 선택, 스크롤, 드릴다운)은 키보드 전용이다.

## 범위

### 포함

1. **마우스 탭 클릭** — 상단 탭 영역 클릭으로 뷰 전환
2. **마우스 리스트 클릭** — Topics / Nodes / Subscribe / Query 결과에서 클릭으로 항목 선택
3. **마우스 휠 스크롤** — 각 뷰의 기본 스크롤 동작(j/k와 동일)에 매핑
4. **`y` yank 복사** — 컨텍스트에 맞는 클립보드 복사
5. **토스트 피드백** — 복사 성공/실패를 상태바에 2초간 표시

### 제외

- 드래그, 리사이즈, 더블클릭, 우클릭 메뉴
- 텍스트 선택 커스텀 구현 (네이티브 터미널의 Shift+드래그로 충분)
- 마우스 모드 토글

## 설계

### 1. 터미널 초기화 (lib.rs)

`ratatui::init()` 대신 수동 초기화로 마우스 캡처를 명시적으로 활성화한다.

- `enable_raw_mode()`
- `stdout().execute(EnterAlternateScreen)`
- `stdout().execute(EnableMouseCapture)`
- panic hook 설치: 패닉 시 `DisableMouseCapture`, `LeaveAlternateScreen`, `disable_raw_mode` 복원
- 정상 종료 시 동일한 복원

### 2. AppEvent에 Mouse 추가 (event.rs)

```rust
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Zenoh(ZenohMessage),
    Tick,
}
```

`EventStream` 루프에서 `Event::Mouse(m)`을 받으면 `AppEvent::Mouse(m)`로 forward. 기존 `Event::Key` + `KeyEventKind::Press` 필터는 유지.

### 3. Hit-testing을 위한 레이아웃 rect 저장

App 구조체에 매 프레임 갱신되는 rect 필드 추가.

```rust
pub tabs_rect: Option<Rect>,
pub tab_rects: [Option<Rect>; 5],
pub list_rect: Option<Rect>,
pub list_first_item_row: u16,
pub list_scroll_offset: usize,
```

- `render()`에서 tabs 영역 전체와 각 탭 제목의 대략적 rect 계산 후 `tab_rects`에 저장
  - 각 탭 제목 너비: `format!("[{}] {}", i+1, TAB_TITLES[i]).len() as u16 + divider("  ")`
  - 누적 x 오프셋으로 개별 rect 계산
- 각 뷰가 리스트를 렌더링할 때 `list_rect`, `list_first_item_row`(border 만큼 +1), `list_scroll_offset`을 app에 기록

### 4. 마우스 핸들러 (app.rs)

```rust
fn handle_mouse(&mut self, ev: MouseEvent) {
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => self.handle_click(ev.column, ev.row),
        MouseEventKind::ScrollUp   => self.handle_wheel_up(),
        MouseEventKind::ScrollDown => self.handle_wheel_down(),
        _ => {}
    }
}
```

**탭 클릭:** `tab_rects[i]`에 좌표 포함되면 `active_view`를 해당 뷰로.

**리스트 클릭:** `list_rect`에 포함되고 `row >= list_first_item_row`면
`index = (row - list_first_item_row) as usize + list_scroll_offset`로 변환. 활성 뷰에 맞게 `topic_selected`, `node_selected`, `sub_selected` 갱신.

**휠:** 활성 뷰별로 처리
- Topics: 리스트 커서 이동
- Subscribe: `sub_selected` 이동(새 설계와 일치)
- Query: 결과 리스트 이동
- Nodes: 리스트 이동
- Dashboard: 무시

### 5. Subscribe 커서 추가

```rust
pub sub_selected: usize,
```

- j/k, 클릭, 휠로 이동
- 새 메시지 도착 시: `sub_selected == 0`이면 그대로 두고(첫 위치 유지), 그 외엔 `sub_selected += 1`로 기존 포커스 유지
- `sub_messages.len()` 클램프
- `ListState::default().with_selected(Some(self.sub_selected))` 사용해 렌더

기존 `sub_scroll` 필드는 제거한다. 선택 기반 스크롤로 대체.

### 6. Yank (클립보드 복사)

**크레이트:** `arboard = "3"` (zemon-tui 의존성 추가)

**키 바인딩** (입력 모드 아닐 때):

| 뷰 | `y` | `Y` |
|---|---|---|
| Topics | 선택 topic의 최신 페이로드 | key_expr |
| Subscribe | `sub_selected` 메시지 페이로드 | 해당 메시지 key_expr |
| Query | 선택된 결과 페이로드 | — |
| Nodes | 선택 노드 zid | — |
| Dashboard | 무시 | 무시 |

**페이로드 직렬화:** `MessagePayload`를 문자열로 변환
- 문자열 payload → 그대로
- 바이너리 payload → `format!("{:02x?}", bytes)` 또는 base64 (간단하게 hex)
- 첨부 attachment는 포함하지 않음 (페이로드 본문만)

**토스트 피드백:**

```rust
pub toast: Option<(String, Instant)>,
```

복사 성공: `Some(("Copied payload (142B)".into(), Instant::now()))`
복사 실패: `Some(("Copy failed: <reason>".into(), Instant::now()))`

상태바 렌더 시 `toast`가 있고 2초 이내면 `NORMAL`/`INPUT` 모드 표시 자리에 토스트 메시지 출력. 2초 지나면 None으로 클리어.

### 7. 의존성

`zemon-tui/Cargo.toml`에 추가:
```toml
arboard = "3"
```

## 데이터 흐름

```
crossterm EventStream
  └→ Event::Mouse → AppEvent::Mouse → App::handle_mouse
                                       ├→ handle_click → tabs/list rect 검사 → state 변경
                                       └→ wheel → 활성 뷰 스크롤

Key 'y'/'Y' → App::handle_view_key → arboard::Clipboard → toast 설정
                                                       └ 실패 시 error toast
```

## 에러 처리

- `arboard::Clipboard::new()` 실패: toast에 "Clipboard unavailable" 표시, 앱은 계속 실행
- 클립보드 set 실패: toast에 에러 메시지 표시
- `Clipboard` 인스턴스는 매 복사마다 생성 (가벼움, 드문 동작). 앱 state에 보관하지 않음
- rect가 아직 None (첫 프레임 전 마우스 이벤트 등): 클릭 무시

## 테스트 전략

zemon은 현재 단위 테스트가 없지만 이 기능은 순수 로직 테스트가 가능한 부분이 많다:

1. **Hit-testing 단위 테스트**: `tab_rects` 주어졌을 때 (col,row) → tab index 변환 함수를 분리해 테스트
2. **리스트 클릭 인덱스 계산**: `list_rect`, `list_first_item_row`, `list_scroll_offset`, click row가 주어졌을 때 인덱스 계산 함수 테스트
3. **Subscribe 커서 유지 로직**: 새 메시지 도착 시 `sub_selected`가 올바르게 갱신되는지 단위 테스트
4. **수동 테스트**: 기존 README의 zenohd + pub 시나리오로 탭 클릭, 리스트 클릭, 휠 스크롤, `y`/`Y`로 복사 확인

단위 테스트는 `arboard`를 호출하지 않는 순수 함수로 유지. 클립보드 실제 동작은 수동 검증.

## 영향 파일

| 파일 | 변경 |
|---|---|
| `crates/zemon-tui/src/lib.rs` | 터미널 init/teardown 수동화, panic hook |
| `crates/zemon-tui/src/event.rs` | `AppEvent::Mouse` 추가, mouse 이벤트 forward |
| `crates/zemon-tui/src/app.rs` | rect 필드, handle_mouse/handle_click, yank, toast, sub_selected, hit-test 함수 |
| `crates/zemon-tui/src/views/dashboard.rs` | (변경 거의 없음, 휠은 무시) |
| `crates/zemon-tui/src/views/topics.rs` | render 시 list_rect 기록 |
| `crates/zemon-tui/src/views/subscribe.rs` | `ListState::with_selected`, `sub_scroll` 제거, list_rect 기록 |
| `crates/zemon-tui/src/views/query.rs` | list_rect 기록 |
| `crates/zemon-tui/src/views/nodes.rs` | list_rect 기록 |
| `crates/zemon-tui/Cargo.toml` | `arboard = "3"` |

## 비호환 / 마이그레이션

- `sub_scroll` 제거 → `sub_selected`로 대체. 키 바인딩은 그대로 (j/k).
- 마우스 캡처 ON으로 기본 터미널 드래그 선택 동작이 변경됨. Shift+드래그로 우회 (문서에 명시).
