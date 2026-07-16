# Examples

Runnable examples of using zenmon for AI-style robot diagnosis. These target a
running [dotori_rcs](https://github.com/gongfour) sim (`AGV_MODE=virtual`), so
they are domain-specific illustrations rather than tests.

| Example | Shows |
|---------|-------|
| [`mission_diagnosis.py`](mission_diagnosis.py) | Trigger a high-level VDA5050 mission with `scenario --task`, let dotori's own planner generate the trajectory, and capture the whole chain (BT → ActionExecutor → planner → follower → motion) as one correlated episode. Uses `--preset stall`, `--track`, and `--contract` schema validation. |

## Prerequisites

```bash
cargo build --release                 # binary at target/release/zenmon
# start zenohd + the dotori_rcs sim so tcp/localhost:7447 is reachable
python3 examples/mission_diagnosis.py
```
