#!/usr/bin/env python3
"""Small official BFCL subset + synthetic AgentRunner state tasks, not a leaderboard.

python scripts/agent_eval.py --fetch --live
Without --live: validate pinned data/selection only. No model or desktop.
"""
from __future__ import annotations

import argparse
import contextlib
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
from itertools import permutations
import subprocess
import tempfile
from urllib.request import urlopen

from companion_eval import parse_rows
from native_uia_smoke import ROOT, build_cargo_binary, isolated_environment

COMMIT = '6ea57973c7a6097fd7c5915698c54c17c5b1b6c8'
BASE = f'https://raw.githubusercontent.com/ShishirPatil/gorilla/{COMMIT}/berkeley-function-call-leaderboard/bfcl_eval/data/'
FILES = {
    'simple.jsonl': ('BFCL_v4_simple_python.json', '82dd63ba502eb2520c6b5d1d9a5c4b590e03ff261565175561f6228a367d1991'),
    'answers.jsonl': ('possible_answer/BFCL_v4_simple_python.json', '90cd5bc653690ee8e459b5b3f3fc9458606f7f3fcbf795bb51b7dc581f8c86dc'),
    'irrelevance.jsonl': ('BFCL_v4_irrelevance.json', '2b6ed4c2e992cdcf5f1678a701851f944bef7550ee026ed1ddb89efed5be01a6'),
}
IDS = ('simple_python_0', 'simple_python_4', 'simple_python_5', 'simple_python_6', 'irrelevance_0', 'irrelevance_3')
HOLDOUT_FILES = {
    'multiple.jsonl': ('BFCL_v4_multiple.json', 'aef168155ebd74b7ac2401198b201343bc7d16d7a3d7e0d4e6d8ee82c6969b2a'),
    'multiple-answers.jsonl': ('possible_answer/BFCL_v4_multiple.json', '244e00ce9395df948bcafc7bee64e8f9c87ef70887587d83cae45b13699f3047'),
    'parallel.jsonl': ('BFCL_v4_parallel.json', '19f51a82eff42e5d62541aa500115a056eb78f437c2ba1f10415fd7c8e5dda84'),
    'parallel-answers.jsonl': ('possible_answer/BFCL_v4_parallel.json', '8a6aa19c1adddc6a5a2f7e40f9dbf30cc7e95815e7b830c90589ab318229e0f0'),
    'parallel_multiple.jsonl': ('BFCL_v4_parallel_multiple.json', '8863ea8433239f55c5f016154cf0830853c89f693c6ea270396a2fa121960579'),
    'parallel_multiple-answers.jsonl': ('possible_answer/BFCL_v4_parallel_multiple.json', '5ebf24f458c1f16300c05505d83d6f0a1b68b79be273a033febd0d4f840507e3'),
}
HOLDOUT_IDS = ('multiple_9', 'multiple_21', 'parallel_1', 'parallel_2', 'parallel_multiple_3', 'parallel_multiple_4', 'irrelevance_4', 'irrelevance_5')
STATE_TASKS = {'smoke': ('delivery_change', 'memory_keyword'), 'holdout': ('delivery_conflict', 'memory_updated')}
ARMS = ('minimal_fc', 'angelbot_prompt')
TEST = 'commands::message::live_tests::agent_benchmarks::configured_agent_benchmarks'
CACHE = ROOT / 'logs/agent-benchmarks-20261027'


def schema(value):
    if isinstance(value, list):
        return [schema(item) for item in value]
    if not isinstance(value, dict):
        return value
    return {key: {'dict': 'object', 'float': 'number'}.get(item, item) if key == 'type' else schema(item)
            for key, item in value.items()}


def source_files(suite):
    return FILES if suite == 'smoke' else {'irrelevance.jsonl': FILES['irrelevance.jsonl']} | HOLDOUT_FILES


def load_cases(fetch=False, suite='smoke'):
    CACHE.mkdir(parents=True, exist_ok=True)
    documents = {}
    for local, (remote, digest) in source_files(suite).items():
        path = CACHE / local
        if not path.exists():
            if not fetch:
                raise RuntimeError('Pinned public data missing; run with --fetch once')
            with urlopen(BASE + remote, timeout=30) as response:
                data = response.read(1024 * 1024)
            if hashlib.sha256(data).hexdigest() != digest:
                raise RuntimeError('Official data hash mismatch')
            path.write_bytes(data)
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise RuntimeError('Cached data hash mismatch; do not silently replace it')
        documents[local] = {row['id']: row for row in map(json.loads, data.decode('utf-8').splitlines())}
    rows, answers = {}, {}
    for name, values in documents.items():
        (answers if 'answers' in name else rows).update(values)
    cases = [rows[key] for key in (IDS if suite == 'smoke' else HOLDOUT_IDS)]
    payload = [{'id': row['id'], 'question': row['question'][0][0]['content'], 'tools': schema(row['function'])} for row in cases]
    return payload, answers


def equal(actual, expected):
    if isinstance(actual, bool) or isinstance(expected, bool):
        return type(actual) is type(expected) and actual == expected
    return actual == expected


def argument_map_matches(actual, expected):
    if not isinstance(actual, dict) or set(actual) - set(expected):
        return False
    return all(any(argument_map_matches(actual[key], option) if isinstance(option, dict) else equal(actual[key], option)
                   for option in options) if key in actual else '' in options
               for key, options in expected.items())


def grade_call(row, answers):
    if row['status'] != 'completed':
        return None
    calls = row['calls']
    if row['id'].startswith('irrelevance_'):
        return not calls and isinstance(row['text'], str) and bool(row['text'].strip())
    gold = [(name, args) for entry in answers[row['id']]['ground_truth'] for name, args in entry.items()]
    if len(calls) != len(gold) or len(gold) > 6 or row.get('argumentsValid') is False:
        return False
    return any(all(call['name'] == name and argument_map_matches(call['arguments'], expected)
                   for call, (name, expected) in zip(calls, order)) for order in permutations(gold))


def grade_state(row):
    if row['status'] in ('unavailable', 'quota_exceeded', 'timed_out', 'invalid_response'):
        return None
    if row['status'] != 'completed':
        return False
    state = row['world']
    if row['id'] in ('delivery_change', 'delivery_conflict'):
        conflict = row['id'] == 'delivery_conflict'
        return state['day'] == 'Thursday' and state['revision'] == (19 if conflict else 18) and state['writes'] == 1 and state['reads_before'] >= (2 if conflict else 1) and state['reads_after'] >= 1
    if row['id'] in ('memory_keyword', 'memory_updated'):
        return state['target_seen'] is True and state['irrelevant_seen'] is False and (row['text'] or '').strip() == ('祁门红茶' if row['id'] == 'memory_updated' else '白毫银针')
    raise ValueError('Unknown synthetic task')


def summarize(rows, answers, suite='smoke', arms=ARMS, static_only=False):
    ids = IDS if suite == 'smoke' else HOLDOUT_IDS
    state_tasks = () if static_only else STATE_TASKS[suite]
    expected = {('bfcl', key, arm) for key in ids for arm in arms} | {('synthetic_state', key, 'agent_runner') for key in state_tasks}
    seen = set()
    for row in rows:
        key = row['kind'], row['id'], row['arm']
        if key not in expected or key in seen or not 0 <= row['requestCount'] <= (1 if row['kind'] == 'bfcl' else 6 if suite == 'holdout' else 4):
            raise ValueError('Invalid, duplicate or over-budget evaluation row')
        seen.add(key)
        row['passed'] = grade_call(row, answers) if row['kind'] == 'bfcl' else grade_state(row)
    groups = {arm: {'passed': sum(row['passed'] is True for row in rows if row['arm'] == arm),
                    'evaluated': sum(row['passed'] is not None for row in rows if row['arm'] == arm),
                    'planned': len(state_tasks) if arm == 'agent_runner' else len(ids)} for arm in (*arms, 'agent_runner')}
    status = 'blocked' if seen != expected or any(row['passed'] is None for row in rows) else 'passed' if all(row['passed'] for row in rows) else 'needs_work'
    return status, groups


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fetch', action='store_true', help='download only missing, SHA256-pinned public BFCL files')
    parser.add_argument('--suite', choices=('smoke', 'holdout'), default='smoke')
    parser.add_argument('--arm', choices=ARMS, help='run only this prompt arm; reuse existing baseline evidence')
    parser.add_argument('--static-only', action='store_true', help='skip synthetic execution tasks; does not claim AgentRunner acceptance')
    parser.add_argument('--live', action='store_true', help='saved Luna only; <=20 smoke / <=28 holdout provider calls, no real tools or desktop')
    args = parser.parse_args()
    arms = (args.arm,) if args.arm else ARMS
    payload, answers = load_cases(args.fetch, args.suite)
    if not args.live:
        print(json.dumps({'status': 'offline_only', 'suite': args.suite, 'arms': arms, 'staticOnly': args.static_only, 'ids': [row['id'] for row in payload], 'dataVerified': True, 'modelScore': None}))
        return 0
    owned = Path(tempfile.mkdtemp(prefix='agent-eval-', dir=ROOT / 'logs'))
    (owned / 'input.json').write_text(json.dumps(payload, ensure_ascii=False), encoding='utf-8')
    report = {'status': 'building', 'checkedAt': datetime.now(timezone.utc).isoformat(),
              'suite': args.suite, 'arms': arms, 'staticOnly': args.static_only,
              'maxProviderCalls': len(payload) * len(arms) + (0 if args.static_only else 2 * (6 if args.suite == 'holdout' else 4)),
              'bfclCommit': COMMIT, 'selectedIds': [row['id'] for row in payload], 'dataSha256': {name: spec[1] for name, spec in source_files(args.suite).items()},
              'sourceSha256': {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in (
                  'scripts/agent_eval.py', 'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs',
                  'src-tauri/src/agent/conversation/prompt.rs', 'src-tauri/src/agent/conversation/runner.rs',
                  'src-tauri/src/llm/openai_responses.rs')},
              'officialLeaderboardScore': False, 'officialTauTasks': False,
              'limits': ['BFCL small fixed subset selected by name/schema compatibility, not random or representative',
                         'Local strict JSON acceptable-value scorer, not the full official AST runner',
                         'Paired arms share AngelBot provider/namespace; only prompt wrapping differs, not whole-framework ablation',
                         'State tasks are authored synthetic fixtures, not official tau-bench or full Main/IPC/desktop acceptance'],
              'results': [], 'summary': {}}
    def record():
        (owned / 'acceptance.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    record()
    print(f'AGENT_EVAL_REPORT {owned / "acceptance.json"}', flush=True)
    try:
        with (owned / 'run.log').open('w', encoding='utf-8') as log:
            with contextlib.redirect_stdout(log), contextlib.redirect_stderr(log):
                binary = build_cargo_binary(library_test=True)
            env = {key: value for key, value in isolated_environment().items() if not key.startswith(('ANGELBOT_', 'VITE_ANGELBOT_'))}
            env.update(ANGELBOT_AGENT_EVAL_LIVE='1', ANGELBOT_PRIVATE_AGENT_EVAL_ROOT=str(owned), ANGELBOT_AGENT_EVAL_SUITE=args.suite)
            if args.arm:
                env['ANGELBOT_AGENT_EVAL_ARM'] = args.arm
            if args.static_only:
                env['ANGELBOT_AGENT_EVAL_STATIC_ONLY'] = '1'
            report['status'] = 'running'
            record()
            log.flush()
            result = subprocess.run([str(binary), TEST, '--exact', '--ignored', '--nocapture', '--test-threads=1'],
                                    cwd=ROOT, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                    timeout=1200, creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        report['results'] = parse_rows((owned / 'run.log').read_text(encoding='utf-8'), marker='AGENT_EVAL_ROW ', test=TEST)
        report['status'], report['summary'] = summarize(report['results'], answers, args.suite, arms, args.static_only)
        if result.returncode:
            report['status'] = 'failed'
    except BaseException:
        report['status'] = 'failed'
        raise
    finally:
        record()
    print(json.dumps({'status': report['status'], 'summary': report['summary']}, ensure_ascii=False), flush=True)
    return 0 if report['status'] == 'passed' else 2


if __name__ == '__main__':
    raise SystemExit(main())
