#!/usr/bin/env python3
"""THROWAWAY LOGIC PROTOTYPE — not imported by AngelBot.

Question: can a small, source-backed record distinguish an ongoing habit,
a situational default, a one-turn exception and a lasting correction?
The inputs below are CURATED structured candidates, not model extractions.
This demonstrates lifecycle/selection only, not natural-language understanding,
long-term model quality or production compatibility. No model, database,
permissions, application control or persistence. No third-party dependency.

Run: python scripts/prototypes/habit_state_prototype.py
Replay: python scripts/prototypes/habit_state_prototype.py --replay
"""

import argparse
import copy
import json
import sys


def initial_state():
    return {"day": 1, "workspace": "personal", "occasion": "work",
            "memory": True, "learning": True, "records": [], "current": {}, "note": "尚无记录"}


def transition(state, event):
    """Pure reducer; the terminal and curated sequence are deliberately separate."""
    state = copy.deepcopy(state)
    kind = event["kind"]
    state["note"] = ""
    if kind == "context":
        state.update({key: event[key] for key in ("workspace", "occasion", "day") if key in event})
        state["current"] = {}
        state["note"] = "新情境／回合；临时例外已清除"
        return state
    if kind == "switch":
        state.update({key: event[key] for key in ("memory", "learning") if key in event})
        state["note"] = "仅调整样例开关；关闭期间不记录，不回溯补学"
        return state
    if kind == "exception":
        state["current"][event["key"]] = event["value"]
        state["note"] = "只覆盖当前回合，不改长期记录"
        return state
    if kind == "revoke":
        for record in state["records"]:
            if record["id"] == event["id"]:
                record["status"] = "disabled"
        state["note"] = "停止沿用；旧来源保留用于拒绝迟到重放"
        return state

    # Candidate classification/slot/condition are curated for this experiment.
    # No heuristics here pretend to understand the raw sentence.
    if event["key"] not in ("reply.detail", "reply.format", "address.name"):
        state["note"] = "拒绝：样例不接受权限、动作或其他槽位"
        return state
    if not (state["memory"] and state["learning"]):
        state["note"] = "未记录：记忆或学习关闭"
        return state
    if event.get("role") != "user" or not event.get("quote") or event["quote"] not in event["text"]:
        state["note"] = "拒绝：没有原用户证据"
        return state
    if any(r["status"] == "disabled" and event["quote"] in r["quotes"] for r in state["records"]):
        state["note"] = "拒绝：撤销记录的旧证据不能复活"
        return state
    if any(event["source"] in r["sources"] for r in state["records"]):
        state["note"] = "同一来源只计一次，不刷新时间或置信度"
        return state

    slot = (event["key"], event.get("workspace", "*"), event.get("occasion", "*"))
    candidates = [r for r in state["records"]
                  if (r["key"], r["workspace"], r["occasion"]) == slot
                  and r["status"] in ("active", "tentative")]
    if kind == "explicit":
        for record in candidates:
            record["status"] = "superseded"
        state["note"] = "长期更正，只替代同槽位、同情境旧记录" if candidates else "明确偏好保留原句，即可沿用"
    else:
        matching = next((r for r in candidates if not r["explicit"] and r["value"] == event["value"]
                         and state["day"] - r["day"] < 90), None)
        if matching:
            # One conversation on one day is one observation, not many votes.
            witness = [event["session"], state["day"]]
            if witness not in matching["witnesses"]:
                matching["sources"].append(event["source"])
                matching["quotes"].append(event["quote"])
                matching["witnesses"].append(witness)
                matching["day"] = state["day"]
                matching["status"] = "active"
                state["note"] = "独立重复观察可成为软性参考，不成为用户命令"
            else:
                state["note"] = "同一会话同一日的观察不增加独立证据"
            return state

    state["records"].append({"id": event["source"], "key": slot[0], "workspace": slot[1],
        "occasion": slot[2], "value": event["value"], "explicit": kind == "explicit",
        "status": "active" if kind == "explicit" else "tentative", "day": state["day"],
        "sources": [event["source"]], "quotes": [event["quote"]],
        "witnesses": [[event["session"], state["day"]]]})
    if kind != "explicit":
        state["note"] = "一次观察保持暂定，不带入回应"
    return state


def working_view(state):
    """Select applicable defaults; current requests are independent of memory."""
    selected = {}
    if state["memory"] and state["learning"]:
        records = [r for r in state["records"] if r["status"] == "active"
                   and (r["explicit"] or state["day"] - r["day"] < 90)
                   and r["workspace"] in ("*", state["workspace"])
                   and r["occasion"] in ("*", state["occasion"])]
        for record in sorted(records, key=lambda r: (r["explicit"], r["workspace"] != "*",
                                                    r["occasion"] != "*", r["day"], r["id"])):
            selected[record["key"]] = {"value": record["value"], "source": record["sources"][-1],
                                      "quote": record["quotes"][-1],
                                      "basis": "explicit" if record["explicit"] else "observed"}
    for key, value in state["current"].items():
        selected[key] = {"value": value, "source": "current-turn", "basis": "exception"}
    return selected


def candidate(source, text, value, *, kind="explicit", role="user", key="reply.detail",
              occasion="work", workspace="*", session="s1"):
    return dict(kind=kind, source=source, session=session, role=role, text=text, quote=text,
                key=key, value=value, occasion=occasion, workspace=workspace)


def sequence():
    # Quotes are evidence fixtures; no reference answer is sent to a model.
    casual = candidate("u-casual-1", "短一点读起来舒服", "brief", kind="observe", occasion="casual")
    correction = candidate("u-correction", "以后工作时改为详细说明", "detailed")
    return [
        ("明确偏好", candidate("u-work", "以后工作时先给简短结论", "brief")),
        ("一次例外", dict(kind="exception", key="reply.detail", value="detailed")),
        ("下一回合恢复原习惯", dict(kind="context", occasion="work")),
        ("长期纠正", correction),
        ("助手推测不算用户证据", candidate("assistant", "你喜欢所有回复很短", "brief", role="assistant")),
        ("一次观察仍暂定", casual),
        ("重复同一证据不升级", casual),
        ("独立后续观察", dict(casual, source="u-casual-2", session="s2")),
        ("进入项目 A", dict(kind="context", workspace="project-A")),
        ("项目内约定", candidate("u-project", "这个项目的结果用表格", "table", key="reply.format", workspace="project-A")),
        ("回到日常不带项目约定", dict(kind="context", workspace="personal")),
        ("闲聊采用闲聊习惯", dict(kind="context", occasion="casual")),
        ("长期无新证据不继续套用推测", dict(kind="context", day=92, occasion="casual")),
        ("明确约定不因观察过期而消失", dict(kind="context", occasion="work")),
        ("撤销明确约定", dict(kind="revoke", id="u-correction")),
        ("后台旧证据不能复活", correction),
        ("关闭学习", dict(kind="switch", learning=False)),
        ("关闭期间不记新称呼", candidate("u-name", "以后叫我小林", "小林", key="address.name", occasion="*")),
        ("重新开启不补学", dict(kind="switch", learning=True)),
        ("关闭长期记忆", dict(kind="switch", memory=False)),
        ("仍遵守当前请求", dict(kind="exception", key="reply.detail", value="brief")),
    ]


def snapshot(state, index, label):
    view = working_view(state)
    return {"prototype": "curated_state_only_not_model_score", "step": index, "scenario": label,
            "state": state, "applied": view,
            "viewUtf8Bytes": len(json.dumps(view, ensure_ascii=False, separators=(",", ":")).encode())}


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--replay", action="store_true", help="print the curated sequence; no model or disk writes")
    args = parser.parse_args()
    state = initial_state()
    steps = sequence()
    index = 0
    while True:
        label = "初始状态" if index == 0 else steps[index - 1][0]
        frame = snapshot(state, index, label)
        if args.replay:
            print(json.dumps(frame, ensure_ascii=False))
        else:
            print("\033[2J\033[H", end="")
            print("THROWAWAY — 人工标注状态样例，不是模型成绩\n")
            print(f"{index}/{len(steps)}  {label} | day={state['day']} {state['workspace']}/{state['occasion']}")
            print(f"memory={state['memory']} learning={state['learning']}\n{state['note']}\n")
            for record in state["records"]:
                effective = "expired" if record["status"] in ("active", "tentative") and not record["explicit"] and state["day"] - record["day"] >= 90 else record["status"]
                print(f"{record['id']}: {record['value']} | {record['workspace']}/{record['occasion']} "
                      f"{effective} day={record['day']} explicit={record['explicit']} sources={record['sources']} "
                      f"quote={record['quotes'][-1]!r}")
            print("\n本轮偏好视图（当前问题正文仍独立保留）：" + json.dumps(frame["applied"], ensure_ascii=False))
            print(f"视图UTF-8字节：{frame['viewUtf8Bytes']}（不是tokens）\n[n/Enter] 下一步  [r] 重置  [q] 退出")
            command = input("> ").strip().lower()
            if command == "q":
                return
            if command == "r":
                state, index = initial_state(), 0
                continue
            if command not in ("", "n"):
                continue
        if index == len(steps):
            if args.replay:
                return
            continue
        state = transition(state, steps[index][1])
        index += 1


if __name__ == "__main__":
    main()
