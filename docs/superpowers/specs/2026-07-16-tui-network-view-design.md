# TUI Network 뷰 & Scout 개념 분리 설계

**날짜:** 2026-07-16
**상태:** 설계 승인됨 (구현 전)
**관련 이슈:** #19 (scout 개념 분리), #20 (토폴로지/관계 뷰)
**범위 밖(후속):** #21 (도움말 오버레이), CLI `domain` 용어 정정(별도 이슈)

## 배경

zenmon TUI에는 성숙한 네트워크 관측 도구(ROS `rqt_graph`, RTI Admin Console, `zenoh-cli network`)가 공통으로 갖춘 **관계/토폴로지 뷰가 없다.** 또한 "scout"이라는 단어가 성격이 다른 두 동작에 겹쳐 쓰여 혼란스럽다:

- **Nodes 뷰 `s`** (`crates/zenmon-tui/src/app.rs:832`): *현재* 스카우팅 네트워크에서 노드를 발견해 목록에 병합.
- **전역 `P` "Scout Port" 모달** (`crates/zenmon-tui/src/app.rs:1088`): *다른* 멀티캐스트 스카우팅 포트를 스캔해 그쪽으로 재접속.

하나는 *현재 화면 갱신*, 하나는 *접속 대상 변경*이다. 같은 단어라 멘탈 모델이 뒤섞인다.

### 용어 확인 (native Zenoh)

공식 문서 확인 결과, **native Zenoh에는 "domain" 개념이 없다.**

- 발견은 멀티캐스트 스카우팅(`224.0.0.224:7446`, 주소·포트 configurable)으로 한다. 기본 포트는 단일 7446.
- 격리는 "도메인"이 아니라 key expression + namespace(프리픽스)로 한다.
- "Domain"은 DDS/ROS2 개념이며, `rmw_zenoh`는 `ROS_DOMAIN_ID`를 key expression 프리픽스로 처리한다(포트로 나누지 않음).
- 따라서 기존 코드의 `domain id = port - 7446` 매핑(`crates/zenmon-cli/src/cli.rs:94`, `crates/zenmon-cli/src/main.rs:62`)은 이 프로젝트가 만든 편의 라벨일 뿐 Zenoh 표준이 아니다.

**결론:** 이 설계는 "domain"을 쓰지 않고 정확한 Zenoh 용어(**scouting port / discovery network**)를 쓴다.

참고:
- Zenoh Deployment(scouting): https://zenoh.io/docs/getting-started/deployment/
- DEFAULT_CONFIG.json5: https://github.com/eclipse-zenoh/zenoh/blob/main/DEFAULT_CONFIG.json5
- Key Expressions RFC: https://github.com/eclipse-zenoh/roadmap/blob/main/rfcs/ALL/Key%20Expressions.md
- rmw_zenoh #201: https://github.com/ros2/rmw_zenoh/issues/201

## 목표

1. 네트워크 관계를 한 화면에서 파악하는 **토폴로지 뷰**를 추가한다 (주 목적: 빠른 건강 체크).
2. "scout"의 두 의미를 명확히 분리하고 정확한 Zenoh 용어로 재명명한다.

## 비목표 (YAGNI)

- 그래프(노드-링크) 렌더링 — 이번엔 router→peer **트리**만.
- 선택 노드 중심 부분 토폴로지 — 이번엔 **전체 지도**만.
- `?` 도움말 오버레이(#21), CLI 용어 정정, 대역폭 지표 — 각각 별도 이슈.
- 코어/CLI 동작 변경 — 이 설계는 **TUI 표시·조작만** 바꾼다. `ZenmonConfig.scout_port` 등은 그대로.

## 채택 접근법: "Nodes" 탭 → "Network" 탭 승격

발견 + 관계 + 스카우팅 포트 전환을 한 곳에 모은다. 대안(별도 Topology 탭 추가 / Dashboard에 미니맵)보다 "네트워크의 집"이 하나로 모여 IA가 깔끔하고, 다른 뷰(Liveliness)와 같은 "좌 목록 + 우 상세" 골격을 재사용한다. 탭 수는 6개 유지.

## 화면 설계

### Network 탭 레이아웃

평면 노드 테이블을 **토폴로지 트리로 대체**한다. 트리가 곧 목록이므로 중복이 없다.

```
┌ zenmon ───────────────────────────────────────────────────────────────────────┐
│ [1] Dashboard  [2] Topics  [3] Stream  [4] Query  [5] Network  [6] Liveliness   │
└─────────────────────────────────────────────────────────────────────────────────┘
┌ Topology — scout:7446 · 4 nodes ── r:refresh ┐┌ Detail — a1b2c3 (router) ────────┐
│ ● router  a1b2c3d4e5f6  192.168.0.5:7447      ││ ZID: a1b2c3d4e5f6a7b8...         │
│  ├─ ● peer    d4e5f6a7  192.168.0.9:41000     ││ Kind: router    Source: both     │
│ >├─ ● peer    99aabbcc  (self)                ││ Locators:                        │
│  └─ ○ client  11882233  192.168.0.20  stale   ││   tcp/192.168.0.5:7447            │
│ ● router  ff00aa11  10.0.0.2:7447  no-sess    ││ Version: 1.9.0                   │
│ ── unlinked (scouted) ──                      ││ Plugins: rest, storage_manager   │
│    ● peer    77788899  192.168.0.31:43000     ││ Sessions (3): ...                │
│                                               ││ Admin seen: now  Scout: 12s ago  │
└───────────────────────────────────────────────┘└──────────────────────────────────┘
 Connected zid:99aabb…  scout:7446  mode:client  NORMAL   q:quit 1-6 r:refresh P:port ?
```

- **좌측 Topology 트리 (~55%)**: 루트=router, 자식=그 router의 Sessions(peer/client). `●` alive / `○` dead·stale, `(self)`, `no-sess`, `stale` 태그. j/k로 노드 행 탐색, 선택 시 우측 갱신.
- **우측 Detail (~45%)**: 기존 Nodes 상세 그대로(ZID/Kind/Source/Locators/Version/Plugins/Sessions/last-seen).
- **상태바**: `scout:7446`(멀티캐스트 스카우팅 포트), 힌트 `r:refresh P:port`. `?`(도움말)는 자리만 표시(#21).

### Switch Scouting Port 모달 (기존 "Scout Port" 모달 개편)

```
        ┌ Switch Scouting Port ──────────────────────┐
        │ Zenoh multicast discovery: 224.0.0.224:P   │
        │ Current: 7446 (default)                    │
        │ Go to port: 7449_                          │
        │                                            │
        │ Scanned ports with nodes:                  │
        │ > 7446   2 node(s)  (self)                 │
        │   7449   1 node(s)                         │
        │   7453   4 node(s)                         │
        │                                            │
        │ s:scan  jk/↑↓:select  Enter:switch  Esc    │
        └────────────────────────────────────────────┘
```

- "domain" 라벨 제거, 스카우팅 포트로 표기. 부제 `224.0.0.224:P`로 "멀티캐스트 발견 포트"임을 명시.
- `s`는 항상 스캔(기존 "입력 비었을 때만" 결합 제거, `crates/zenmon-tui/src/app.rs:447`).

## Scout 개념 분리 (키·용어)

| 지금 | 바뀜 | 의미 (native Zenoh) |
|------|------|------|
| Nodes `s` | `r` = refresh (Network 뷰) | 현재 스카우팅 네트워크에서 노드 재발견(멀티캐스트 scout) |
| 전역 `P` "Scout Port" | `P` = Switch Scouting Port (전역, `m:mode` 옆) | 접속할 멀티캐스트 스카우팅 포트(발견 네트워크) 전환 후 재접속 |

`r`은 기존 `pending_scout_request` 메커니즘을 이름만 바꿔 재사용. admin space는 이미 주기적으로 자동 갱신되어 트리에 반영된다.

## 토폴로지 트리 모델

데이터 원천: `app.nodes = merge_nodes(admin_nodes, scout_nodes)`. 각 `NodeInfo`에 `zid / kind / locators / metadata.sessions / sources / *_last_seen`.

**구축 규칙**

1. 루트 = `kind == "router"` 노드.
2. 자식 = 그 router의 `metadata.sessions[]`(peer zid / whatami / link dst). session peer zid를 `app.nodes`에서 조회해 kind·locator·stale을 풍부히 표시, 없으면 session 정보만으로 렌더.
3. `── unlinked (scouted) ──` = router가 아니면서 어떤 router의 자식으로도 안 나타난 노드.
4. router가 하나도 없으면 전 노드를 최상위 평면 표시(헤더 `── nodes (no router) ──`).
5. 한 peer가 여러 router에 붙었으면 각 router 밑에 중복 표기. 단일 router 자식 내에서는 중복 제거.

**건강 표시** (기존 로직 재사용): `●`=alive, `○`=dead/stale(`is_scout_stale`, 30s), `no-sess`, `stale`, `(self)`.

## 빈 상태 · 엣지 케이스

1. 노드 없음: 트리에 "No nodes yet — press `r` to scout", Detail "No node selected".
2. router 없음(peer-only): 규칙 4의 평면 표시.
3. 세션 없는 router: `no-sess` 태그, 자식 없음.
4. scout-only 노드: router면 루트로, 아니면 unlinked 그룹.
5. NodeInfo 없는 session 자식: Detail에 최소 정보(zid·whatami·link addr) + "(from session metadata; limited detail)".
6. Switch Scouting Port 스캔 결과 없음: "No nodes found in scanned ports". 잘못된 포트 입력: 기존 error toast.
7. 연결 끊김/재접속: 기존 `clear_network_state`가 nodes를 비워 트리 자동 초기화.

## 탐색·선택 구현

- 트리를 **평탄화한 행 목록**으로 렌더. 헤더 행은 선택 불가(j/k·클릭이 건너뜀).
- `node_selected`는 "선택 가능한 노드 행" 인덱스. 데이터 변경 시 clamp.
- 클릭 히트테스트(`list_hit`)는 헤더를 건너뛰도록 행→노드 매핑. 스크롤은 기존 `list_scroll_offset` 재사용.

## 코드 구조 · 이름 변경

- `ActiveView::Nodes` → `ActiveView::Network`, `TAB_TITLES` 갱신.
- `crates/zenmon-tui/src/views/nodes.rs` → `views/network.rs`.
- 트리 구축은 순수 함수 `build_topology_rows(&[NodeInfo], self_zid: Option<&str>) -> Vec<TopoRow>` 로 분리(뷰 모듈). `app.rs`는 선택 인덱스·키 처리만 담당(app.rs가 이미 1604줄이라 렌더/모델 분리 유지).
- `TopoRow` 열거형: `Header { label }` | `Node { zid, depth, kind, locator, alive, stale, is_self, is_child }`.

## 테스트

기존 관례대로 렌더링은 미테스트, 순수 로직만 단위 테스트:

- `build_topology_rows`:
  - router→sessions 자식 구성
  - 두 router에 걸친 peer 중복 표기
  - 비-router·비-자식 → unlinked 그룹
  - router 없음 → no-router 평면
  - self·stale 마커
- 선택 이동(`next/prev_selectable`)이 헤더를 건너뛰는지.
- 회귀 방지: 새 모듈에 `"domain"` 문자열 부재 확인(용어 정확성).

## 후속 과제 (이 스펙 범위 밖)

- **CLI `domain` 용어 정정**: `zenmon scout`가 `(domain N)`을 출력(`crates/zenmon-cli/src/cli.rs:94`, `crates/zenmon-cli/src/main.rs:62`). TUI만 고치면 CLI와 어긋나므로 별도 이슈로 정정.
- **#21 도움말 오버레이**: `?` 키 + 뷰별 동적 힌트.
