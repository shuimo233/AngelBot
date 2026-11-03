#!/usr/bin/env python3
"""Minimal memory-to-response probe. Default: offline, no credentials or model.

python scripts/companion_eval.py --live
Uses only saved ChatGPT/Luna, no tools, desktop, user memory or API fallback.
Fixtures are synthetic benchmark-inspired probes, NOT official benchmark scores.
"""
from __future__ import annotations

import argparse
import contextlib
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from native_uia_smoke import ROOT, build_cargo_binary, isolated_environment

TEST = 'commands::message::live_tests::companion::minimal_companion_probe'
FIXTURES = {name: ROOT / f'scripts/fixtures/companion_eval{suffix}.json' for name, suffix in (('smoke', ''), ('holdout', '_holdout'))}
ARMS = ('oracle_history', 'angelbot_recall')


def parse_rows(log, *, marker='COMPANION_RESULT ', test=TEST):
    prefixes = (marker, f'test {test} ... {marker}')
    rows = []
    for line in log.splitlines():
        for prefix in prefixes:
            if line.startswith(prefix):
                rows.append(json.loads(line.removeprefix(prefix)))
                break
    return rows


def summarize_metering(rows):
    """Aggregate actual request records; absent historical fields remain unknown."""
    requests = [request for row in rows for request in row.get('requestMetrics', [])]
    attempted = sum(row.get('requestCount', 0) for row in rows)
    reported = [r for r in requests if r.get('usageStatus') == 'reported' and isinstance(r.get('usage'), dict)]
    def observed(field):
        return [r['usage'][field] for r in reported
                if not r['usage'].get('detailsInvalid', False) and type(r['usage'].get(field)) is int]
    cache_rows = [r for r in reported if not r['usage'].get('detailsInvalid', False)
                  and type(r['usage'].get('cachedInputTokens')) is int]
    cached = sum(r['usage']['cachedInputTokens'] for r in cache_rows)
    denominator = sum(r['usage']['inputTokens'] for r in cache_rows)
    latencies = [r['latencyMs'] for r in requests if type(r.get('latencyMs')) in (int, float)]
    result = {'attemptedRequests': attempted, 'requestRecords': len(requests),
              'reportedUsageRequests': len(reported), 'usageComplete': len(reported) == attempted and attempted > 0,
              'cacheReadKnownRequests': len(cache_rows),
              'cacheReadCoverage': len(cache_rows) / attempted if attempted else None,
              'cachedInputTokensObserved': cached if cache_rows else None,
              'cacheDenominatorInputTokens': denominator if cache_rows else None,
              'tokenCacheHitRateObserved': cached / denominator if denominator else None,
              'requestCacheHitRateObserved': sum(r['usage']['cachedInputTokens'] > 0 for r in cache_rows) / len(cache_rows) if cache_rows else None,
              'cacheComplete': len(cache_rows) == attempted and attempted > 0,
              'requestLatencySamplesMs': latencies, 'firstTokenMs': None,
              'p95Ms': None, 'p95Status': 'raw samples only; no independent load repetitions',
              'actualCostUsd': None, 'costStatus': 'not_observed; API prices do not establish plan charges'}
    for field in ('cacheWriteTokens', 'cacheWrite30mTokens', 'cacheWrite12hTokens', 'uncachedInputTokens', 'reasoningTokens'):
        values = observed(field)
        result[field + 'KnownRequests'] = len(values)
        result[field + 'Observed'] = sum(values) if values else None
    return result


def score(cases, rows, live):
    expected = {(case['id'], arm): case for case in cases for arm in ARMS}
    seen = set()
    for row in rows:
        key = (row['id'], row['arm'])
        if key not in expected or key in seen:
            raise ValueError('Unknown or duplicate probe result')
        seen.add(key)
        if row['toolCount'] != 0 or row['requestCount'] != int(live):
            raise ValueError('Unexpected tool or model request count')
        if row['status'] not in (('completed', 'quota_exceeded', 'unavailable', 'timed_out', 'invalid_response') if live else ('offline',)):
            raise ValueError('Invalid probe status')
        case = expected[key]
        evidence = set(row.get('evidenceSourceIndices', []))
        row['evidencePassed'] = (set(case.get('requiredEvidence', [])) <= evidence and not evidence.intersection(case.get('forbiddenEvidence', []))) if row['arm'] == 'angelbot_recall' else None
        row['answerPassed'] = (row['answer'].strip() in case['expected']) if row['status'] == 'completed' and isinstance(row['answer'], str) else None
        row['passed'] = (row['answerPassed'] and row['evidencePassed'] is not False) if row['answerPassed'] is not None else None
    blocked = any(row['status'] not in ('completed', 'offline') for row in rows)
    summary = {arm: {'passed': sum(row['passed'] is True for row in rows if row['arm'] == arm),
                     'evaluated': sum(row['passed'] is not None for row in rows if row['arm'] == arm),
                     'planned': len(cases)} for arm in ARMS}
    status = 'blocked' if blocked or seen != set(expected) else 'offline_only' if not live else 'passed' if all(row['passed'] is True for row in rows) else 'needs_work'
    return status, summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live', action='store_true', help='explicitly invoke the saved Luna model, at most 20 provider calls; stop on first provider failure')
    parser.add_argument('--suite', choices=FIXTURES, default='smoke', help='smoke is the original diagnostic; holdout is the pre-fixed new acceptance set')
    args = parser.parse_args()
    fixture_path = FIXTURES[args.suite]
    fixture = json.loads(fixture_path.read_text(encoding='utf-8'))
    cases = fixture['cases']
    # Gold answers and rubric labels never cross the inference boundary.
    payload = [{key: case[key] for key in ('id', 'sources', 'question')} for case in cases]
    (ROOT / 'logs').mkdir(exist_ok=True)
    owned = Path(tempfile.mkdtemp(prefix='companion-eval-', dir=ROOT / 'logs'))
    input_path = owned / 'input.json'
    input_path.write_text(json.dumps(payload, ensure_ascii=False), encoding='utf-8')
    report = {'status': 'building', 'checkedAt': datetime.now(timezone.utc).isoformat(),
              'mode': 'live' if args.live else 'offline', 'suite': args.suite, 'fixtureSha256': hashlib.sha256(fixture_path.read_bytes()).hexdigest(),
              'sourceSha256': {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in (
                  'scripts/companion_eval.py', 'src-tauri/src/commands/message_live_tests/companion.rs',
                  'src-tauri/src/commands/message.rs', 'src-tauri/src/agent/conversation/prompt.rs',
                  'src-tauri/src/fts.rs', 'src-tauri/src/agent/extensions/memory.rs')},
              'officialBenchmark': False, 'scope': 'preloaded source utterances -> reopened synthetic SQLite -> production recall/prompt -> single text inference per arm',
              'limits': ['No automatic memory extraction, complete Main tool loop or learning-hook acceptance',
                         'Oracle history is a supplied-evidence control, not a matched-token ablation',
                         'Decision-choice correctness is not natural conversation quality or a longitudinal companionship result'],
              'results': [], 'summary': {}}
    report_path = owned / 'acceptance.json'
    def record():
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    record()
    print(f'COMPANION_REPORT {report_path}', flush=True)
    try:
        with (owned / 'run.log').open('w', encoding='utf-8') as log:
            with contextlib.redirect_stdout(log), contextlib.redirect_stderr(log):
                binary = build_cargo_binary(library_test=True)
            env = {key: value for key, value in isolated_environment().items() if not key.startswith(('ANGELBOT_', 'VITE_ANGELBOT_'))}
            env.update(ANGELBOT_COMPANION_MODE=report['mode'], ANGELBOT_PRIVATE_COMPANION_INPUT=str(input_path))
            if args.live:
                env['ANGELBOT_LIVE_MODEL_TEST'] = '1'
            else:
                config_root = owned / 'empty-config'
                config_root.mkdir()
                env['ANGELBOT_PRIVATE_VERIFY_CONFIG_DIR'] = str(config_root)
            report['status'] = 'running'
            record()
            log.flush()
            result = subprocess.run([str(binary), TEST, '--exact', '--ignored', '--nocapture', '--test-threads=1'],
                                    cwd=ROOT, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                    timeout=1000, creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        report['results'] = parse_rows((owned / 'run.log').read_text(encoding='utf-8'))
        report['status'], report['summary'] = score(cases, report['results'], args.live)
        if result.returncode:
            report['status'] = 'failed'
    except BaseException:
        report['status'] = 'failed'
        raise
    finally:
        record()
    print(json.dumps({'status': report['status'], 'summary': report['summary']}, ensure_ascii=False), flush=True)
    return 0 if report['status'] in ('passed', 'offline_only') else 2


if __name__ == '__main__':
    raise SystemExit(main())
