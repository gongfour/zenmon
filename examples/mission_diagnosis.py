#!/usr/bin/env python3
"""Example: AI-style mission diagnosis with `zenmon scenario`.

Triggers a high-level VDA5050 mission on a running dotori_rcs stack and records
the *entire* resulting causal chain — mission_manager -> behaviour tree ->
ActionExecutor -> the planner-generated trajectory -> trajectory_follower ->
motion — as one correlated episode an AI can reason over. It sends only the goal
(navigate N1 -> N2); dotori's own planner produces the trajectory.

What it demonstrates:
  * `scenario --task` triggers a task and follows its feedback/response.
  * `--preset stall` observes the mission-diagnosis topic set (safety, obstacles,
    mission state, actionflow/BT, task feedback/response, pose, forklift snapshot).
  * `--track KEY:FIELD` turns pose into a ready-made series/delta.
  * `--contract` + `-n <fleet>` surfaces & validates the mission request schema.

Prerequisites:
  * `cargo build --release` (binary at target/release/zenmon)
  * A running dotori_rcs sim (AGV_MODE=virtual) reachable at tcp/localhost:7447
  * contracts/dotori_rcs.contract.yaml present

Run:
  python3 examples/mission_diagnosis.py
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = str(ROOT / "target" / "release" / "zenmon")
CONTRACT = str(ROOT / "contracts" / "dotori_rcs.contract.yaml")
NS = ["-n", "dotori/forky001"]  # fleet namespace: keys are relative to it


def zenmon(*args, **kw):
    return subprocess.run([BIN, *args], text=True, capture_output=True, **kw)


def current_pose():
    out = zenmon(*NS, "--json", "sub", "topic/navigation/robot_pose",
                 "--count", "1", "--duration", "3s").stdout
    for line in out.splitlines():
        if line.strip():
            return json.loads(line)["payload"]
    return None


def build_mission(pose):
    """A minimal 2-node VDA5050 order: navigate 2 m forward along +y."""
    x0, y0, th = pose["x"], pose["y"], pose["theta"]
    x1, y1 = x0, y0 + 2.0

    def at(x, y):
        return {"x": x, "y": y, "theta": th, "map_id": "",
                "allowed_deviation_xy": 0.5, "allowed_deviation_theta": 0.5}

    return {
        "mission_id": "zenmon-example-mission",
        "mission": {
            "order_id": "zenmon-example-order",
            "order_update_id": 0,
            "steps": [
                {"type": "waypoint", "node_id": "N1", "sequence_id": 0,
                 "released": True, "position": at(x0, y0)},
                {"type": "traversal", "edge_id": "E1", "sequence_id": 1,
                 "released": True, "start_node_id": "N1", "end_node_id": "N2",
                 "start_x": x0, "start_y": y0, "start_theta": th,
                 "end_x": x1, "end_y": y1, "end_theta": th,
                 "map_id": "", "max_speed": 0.3, "direction": "forward"},
                {"type": "waypoint", "node_id": "N2", "sequence_id": 2,
                 "released": True, "position": at(x1, y1)},
            ],
        },
    }


def main():
    if zenmon("--json", "doctor", "--timeout", "4s").returncode != 0:
        sys.exit("router unreachable at tcp/localhost:7447 — start zenohd + dotori_rcs first")

    pose = current_pose()
    if pose is None:
        sys.exit("no robot_pose — is pose_publisher running?")
    print(f"current pose: x={pose['x']:.2f} y={pose['y']:.2f} theta={pose['theta']:.2f}")

    mission = build_mission(pose)
    print("mission: N1 --E1--> N2 (navigate +2m)")

    proc = zenmon(
        *NS, "--contract", CONTRACT, "--json", "scenario",
        "--task", "task/mission/mission", json.dumps(mission),
        "--preset", "stall",
        "--track", "topic/navigation/robot_pose:x",
        "--track", "topic/navigation/robot_pose:y",
        "--for", "40s", "--settle", "2s",
    )
    episode = json.loads(proc.stdout)

    # --- Diagnosis summary (what an AI would read) ---
    meta = episode["meta"]
    print(f"\nended: {meta['ended_reason']}  ({meta['message_count']} messages, "
          f"{len(episode['correlations'])} correlation chains)")

    def last_payload(suffix):
        hits = [e for e in episode["timeline"] if e["key_expr"].endswith(suffix)]
        return hits[-1]["payload"] if hits else None

    resp = last_payload("task/mission/mission/response")
    if resp:
        print(f"mission: {resp.get('status')} "
              f"(success={resp.get('response', {}).get('success')})")

    traj = [e for e in episode["timeline"] if "trajectory/feedback" in e["key_expr"]]
    print(f"planner trajectory: {len(traj)} feedback events "
          f"(dotori planned the path, not us)")

    safety = last_payload("topic/safety/safety_state")
    if safety:
        print(f"safety: kind={safety.get('kind')} "
              f"causes={safety.get('active_causes')} (0/empty = no stop)")

    tracks = episode.get("tracks", {})
    for f in ("x", "y"):
        t = tracks.get(f"topic/navigation/robot_pose:{f}")
        if t and t.get("first") is not None:
            print(f"pose {f}: {t['first']:.3f} -> {t['last']:.3f} (delta={t['delta']:+.3f} m)")


if __name__ == "__main__":
    main()
