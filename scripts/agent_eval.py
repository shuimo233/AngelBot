#!/usr/bin/env python3
"""Small official BFCL subset + synthetic AgentRunner state tasks, not a leaderboard.

python scripts/agent_eval.py --fetch --live
Without --live: validate pinned data/selection only. No model or desktop.
--suite workspace: evaluate two real file reads using disposable inputs.
--suite daily: evaluate real file reads, scoped recall, and one verified artifact.
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
CASE_IDS = {'smoke': IDS, 'holdout': HOLDOUT_IDS, 'workspace': (), 'daily': ()}
STATE_TASKS = {'smoke': ('delivery_change', 'memory_keyword'), 'holdout': ('delivery_conflict', 'memory_updated'),
               'workspace': ('workspace_pair',), 'daily': ('daily_memory_delivery',)}
STATE_REQUEST_BUDGET = {'smoke': 4, 'holdout': 6, 'workspace': 4, 'daily': 6}
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
    if not CASE_IDS[suite]:
        return {}
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
    cases = [rows[key] for key in CASE_IDS[suite]]
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


def call_checks(row, answers):
    if row['status'] != 'completed':
        return None
    calls = row['calls']
    if row['id'].startswith('irrelevance_'):
        return {'no_tool_call': not calls, 'nonempty_reply': isinstance(row['text'], str) and bool(row['text'].strip())}
    gold = [(name, args) for entry in answers[row['id']]['ground_truth'] for name, args in entry.items()]
    checks = {'call_count': len(calls) == len(gold), 'schema_accepted': row.get('argumentsValid') is not False}
    if all(checks.values()):
        checks['reference_calls'] = len(gold) <= 6 and any(
            all(call['name'] == name and argument_map_matches(call['arguments'], expected)
                for call, (name, expected) in zip(calls, order))
            for order in permutations(gold)
        )
    return checks


def daily_json(text):
    def unique_object(pairs):
        result = dict(pairs)
        if len(result) != len(pairs):
            raise ValueError('Duplicate JSON key')
        return result

    try:
        return json.loads(text, object_pairs_hook=unique_object)
    except (TypeError, ValueError):
        return None


def daily_payload_matches(value):
    expected = {'east': 247, 'west': 389, 'total': 636, 'heading': '青灯月报'}
    return type(value) is dict and value.keys() == expected.keys() and all(
        type(value[key]) is type(want) and value[key] == want for key, want in expected.items())


def daily_files(root):
    """Read the disposable fixture's files independently of Rust receipts."""
    def read(name, limit):
        path = root / name
        try:
            if path.is_symlink() or path.resolve(strict=True).parent != root.resolve(strict=True):
                return None
            with path.open('rb') as handle:
                data = handle.read(limit + 1)
            return data if len(data) <= limit else None
        except OSError:
            return None

    original = all(read(name, len(raw)) == raw for name, raw in (
        ('east.json', b'{"total":247}\n'), ('west.json', b'{"total":389}\n')))
    return {'inputsPreserved': original, 'artifact': daily_json(read('invoice-summary.json', 4096))}


def state_checks(row):
    """Score observable outcomes independently; no reference tool sequence."""
    if row['status'] in ('unavailable', 'quota_exceeded', 'timed_out', 'invalid_response'):
        return None
    state = row['world'] or {}
    text = (row['text'] or '').strip()
    checks = {'runner_completed': row['status'] == 'completed'}
    if row['id'] in ('delivery_change', 'delivery_conflict'):
        conflict = row['id'] == 'delivery_conflict'
        return checks | {
            'delivery_day': state.get('day') == 'Thursday',
            'delivery_revision': state.get('revision') == (19 if conflict else 18),
            'single_write': equal(state.get('writes'), 1),
            'read_before_update': state.get('reads_before', 0) >= (2 if conflict else 1),
            'read_after_update': state.get('reads_after', 0) >= 1,
            'final_answer': text.casefold() == 'thursday',
        }
    if row['id'] in ('memory_keyword', 'memory_updated'):
        checks.update({
            'relevant_record_read': state.get('target_seen') is True,
            'no_unrelated_record_read': state.get('irrelevant_seen') is False,
            'final_answer': text == ('祁门红茶' if row['id'] == 'memory_updated' else '白毫银针'),
        })
        if row['id'] == 'memory_updated':
            checks['superseded_record_hidden'] = state.get('superseded_seen') is False
        return checks
    if row['id'] in ('workspace_pair', 'daily_memory_delivery'):
        if row['id'] == 'daily_memory_delivery':
            disk = row.get('disk') or {}
            return checks | {
                'live_request_budget': type(row.get('requestCount')) is int and 1 <= row['requestCount'] <= STATE_REQUEST_BUDGET['daily'],
                'both_inputs_read': set(state.get('readFiles', [])) == {'east.json', 'west.json'},
                'inputs_preserved': state.get('inputsPreserved') is True and disk.get('inputsPreserved') is True,
                'current_memory_only': state.get('target_seen') is True and state.get('superseded_seen') is False and state.get('irrelevant_seen') is False,
                'recall_before_write': state.get('recallBeforeWrite') is True,
                'single_write': equal(state.get('writes'), 1) and equal(state.get('writeDispatches'), 1),
                'saved_artifact': daily_payload_matches(disk.get('artifact')) and daily_payload_matches(state.get('artifact')),
                'artifact_read_back': state.get('artifactReadBack') is True,
                'final_answer': daily_payload_matches(daily_json(text)),
            }
        try:
            answer = json.loads(text)
        except ValueError:
            answer = None
        expected = {'east': 247, 'west': 389, 'total': 636}
        return checks | {
            'both_inputs_read': set(state.get('readFiles', [])) == {'east.json', 'west.json'},
            'inputs_preserved': state.get('inputsPreserved') is True,
            'final_answer': answer == expected,
        }
    raise ValueError('Unknown synthetic task')


def summarize(rows, answers, suite='smoke', arms=ARMS, static_only=False):
    ids = CASE_IDS[suite]
    state_tasks = () if static_only else STATE_TASKS[suite]
    expected = {('bfcl', key, arm) for key in ids for arm in arms} | {('synthetic_state', key, 'agent_runner') for key in state_tasks}
    seen = set()
    for row in rows:
        key = row['kind'], row['id'], row['arm']
        if key not in expected or key in seen or not 0 <= row['requestCount'] <= (1 if row['kind'] == 'bfcl' else STATE_REQUEST_BUDGET[suite]):
            raise ValueError('Invalid, duplicate or over-budget evaluation row')
        seen.add(key)
        checks = call_checks(row, answers) if row['kind'] == 'bfcl' else state_checks(row)
        row['checks'] = checks
        row['passed'] = None if checks is None else all(checks.values())
    groups = {arm: {'passed': sum(row['passed'] is True for row in rows if row['arm'] == arm),
                    'evaluated': sum(row['passed'] is not None for row in rows if row['arm'] == arm),
                    'planned': len(state_tasks) if arm == 'agent_runner' else len(ids)} for arm in (*arms, 'agent_runner')}
    status = 'blocked' if seen != expected or any(row['passed'] is None for row in rows) else 'passed' if all(row['passed'] for row in rows) else 'needs_work'
    return status, groups


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fetch', action='store_true', help='download only missing, SHA256-pinned public BFCL files')
    parser.add_argument('--suite', choices=tuple(CASE_IDS), default='smoke')
    parser.add_argument('--arm', choices=ARMS, help='run only this prompt arm; reuse existing baseline evidence')
    parser.add_argument('--static-only', action='store_true', help='skip synthetic execution tasks; does not claim AgentRunner acceptance')
    parser.add_argument('--live', action='store_true', help='saved Luna only; <=20 smoke / <=28 holdout / <=4 workspace / <=6 daily provider calls')
    args = parser.parse_args()
    has_bfcl = bool(CASE_IDS[args.suite])
    if not has_bfcl and (args.arm or args.static_only):
        parser.error(f'{args.suite} runs only the AgentRunner task')
    arms = ((args.arm,) if args.arm else ARMS) if has_bfcl else ()
    payload, answers = load_cases(args.fetch, args.suite)
    if not args.live:
        print(json.dumps({'status': 'offline_only', 'suite': args.suite, 'arms': arms, 'staticOnly': args.static_only, 'ids': [row['id'] for row in payload], 'dataVerified': has_bfcl, 'modelScore': None}))
        return 0
    owned = Path(tempfile.mkdtemp(prefix='agent-eval-', dir=ROOT / 'logs'))
    (owned / 'input.json').write_text(json.dumps(payload, ensure_ascii=False), encoding='utf-8')
    report = {'status': 'building', 'checkedAt': datetime.now(timezone.utc).isoformat(),
              'suite': args.suite, 'arms': arms, 'staticOnly': args.static_only,
              'stateRubricVersion': 3,
              'maxProviderCalls': len(payload) * len(arms) + (0 if args.static_only else len(STATE_TASKS[args.suite]) * STATE_REQUEST_BUDGET[args.suite]),
              'bfclCommit': COMMIT, 'selectedIds': [row['id'] for row in payload], 'dataSha256': {name: spec[1] for name, spec in source_files(args.suite).items()},
              'sourceSha256': {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in (
                  'scripts/agent_eval.py', 'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs',
                  'src-tauri/src/agent/conversation/prompt.rs', 'src-tauri/src/agent/conversation/runner.rs',
                  'src-tauri/src/agent/tools/registry.rs', 'src-tauri/src/agent/tools/tool.rs',
                  'src-tauri/src/agent/tools/handlers.rs', 'src-tauri/src/commands/file.rs', 'src-tauri/src/commands/memory.rs',
                  'src-tauri/src/agent/extensions/memory.rs', 'src-tauri/src/fts.rs', 'src-tauri/src/llm/openai_responses.rs')},
              'officialLeaderboardScore': False, 'officialTauTasks': False,
              'limits': ['BFCL small fixed subset selected by name/schema compatibility, not random or representative',
                         'Local strict JSON acceptable-value scorer, not the full official AST runner',
                         'Paired arms share AngelBot provider/namespace; only prompt wrapping differs, not whole-framework ablation',
                         'Memory tasks use the production recall tool with synthetic in-memory records',
                         'The workspace case uses the production file reader in a disposable project root',
                         'The daily case combines scoped recall, real project files and verified artifact delivery',
                         'State tasks are authored fixtures, not official tau-bench or full Main/IPC/desktop acceptance'],
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
        if args.suite == 'daily':
            for row in report['results']:
                row['disk'] = daily_files(owned / 'workspace-pair')
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
