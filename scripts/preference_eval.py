#!/usr/bin/env python3
"""Opt-in frozen PrefEval MCQ memory-component experiment, not a leaderboard score.

python scripts/preference_eval.py --split development
python scripts/preference_eval.py --split development --live
Official CC BY-NC data stays in ignored cache; no download, model or owner data
access by default. No tools, desktop, paid API fallback, or production deployment.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import tempfile

from context_eval import ROOT, run_context_worker

SOURCE = ROOT / 'logs/preference-benchmark-20261028-source'
MANIFEST_SHA = '7a86b6ffeaf5e51fdc79948a83a5d005bf04d267cf36a0dd428e9ea6efee679a'
REVISION = '50795054b5ff5f418d2b768a331d71e480f93331'
TEST = 'commands::message::live_tests::preference_benchmark::configured_preference_benchmark'
ARMS = ('no_history', 'history_reader', 'stored_defaults')
LEARNING_ARMS = ('stored_defaults', 'main_grounded', 'main_memory')


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def project_case(case):
    """Only original conversation, question and shuffled choices cross to Rust."""
    if not re.fullmatch(r'prefeval\.[a-z_]+\.(explicit|implicit_choice)\.\d{4}', case['id']):
        raise ValueError('Unknown case identity')
    history = case['history']
    if not history or len(history) > 20 or any(
            set(s) != {'role', 'content'} or s['role'] not in ('user', 'assistant')
            or not isinstance(s['content'], str) or len(s['content']) > 1000 for s in history):
        raise ValueError('Invalid or oversized source history; do not silently truncate')
    if len(case['options']) != 4 or any(not isinstance(o, str) for o in case['options']) or len(set(case['options'])) != 4:
        raise ValueError('Four original shuffled options are required')
    if not isinstance(case['question'],str) or not case['question'].strip():
        raise ValueError('A source question is required')
    return {key: case[key] for key in ('id', 'history', 'question', 'options')}


def load_source(split):
    manifest_path = SOURCE / 'manifest.json'
    if digest(manifest_path) != MANIFEST_SHA:
        raise ValueError('Frozen source manifest changed; do not select a new sample after seeing answers')
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    if manifest['revision'] != REVISION or manifest['license'] != 'CC-BY-NC-4.0':
        raise ValueError('Unexpected source revision or license')
    names = (f'inputs/{split}.json', f'scoring/{split}.truth.json')
    files = {f['path']: f for f in manifest['outputs']}
    for name in names:
        if digest(SOURCE / name) != files[name]['sha256']:
            raise ValueError(f'Frozen benchmark artifact changed: {name}')
    source, rubric = [json.loads((SOURCE / name).read_text(encoding='utf-8')) for name in names]
    cases = [project_case(case) for case in source['cases']]
    truth = {case['id']: case['correct_label'] for case in rubric['cases']}
    if len(cases) != 4 or len(truth) != 4 or {c['id'] for c in cases} != set(truth):
        raise ValueError('Incomplete or duplicate fixed sample')
    if any(answer not in 'ABCD' or len(answer) != 1 for answer in truth.values()):
        raise ValueError('Invalid private scoring label')
    return cases, truth


def choice(text):
    matches = re.findall(r'<choice>\s*([A-D])\s*</choice>', text or '')
    return matches[0] if len(matches) == 1 else None


def score(cases, truth, rows, arms):
    expected = {(c['id'], arm, phase) for c in cases for arm in arms
                for phase in (('learn', 'answer') if arm in LEARNING_ARMS else ('answer',))}
    seen = set()
    models = {r.get('model') for r in rows}
    if len(models) > 1 or any(not isinstance(model,str) or not model for model in models):
        raise ValueError('Model binding changed during the comparison')
    for row in rows:
        key = (row['id'], row['arm'], row['phase'])
        if key not in expected or key in seen or row.get('requestCount') != 1:
            raise ValueError('Unknown, duplicate or over-budget worker result')
        seen.add(key)
        if row.get('toolCount', 0) != 0:
            raise ValueError('Unexpected tool execution')
        if row['arm'] in ('main_grounded','main_memory') and row['phase']=='answer' and row['status']=='completed':
            if row.get('durableStatus')!='completed':
                raise ValueError('Main response lacks durable completion')
            if row['arm']=='main_memory' and (not row.get('sourceSession') or not row.get('session')
                    or row['sourceSession']==row['session']
                    or row.get('workingView',{}).get('originalMessages')!=0
                    or row.get('workingView',{}).get('workingMessages')!=0):
                raise ValueError('Memory-only Main retained the original conversation')
        if row['phase'] == 'answer':
            row['choice'] = choice(row.get('answer')) if row['status'] == 'completed' else None
            row['correct'] = row['choice'] == truth[row['id']]
    summary = {arm: {'correct': sum(r.get('correct') is True for r in rows if r['arm'] == arm),
                     'answered': sum(r['phase'] == 'answer' and r['status'] == 'completed' for r in rows if r['arm'] == arm),
                     'planned': len(cases)} for arm in arms}
    if 'stored_defaults' in summary:
        summary['stored_defaults']['withDurablePreferences'] = sum(
            r['arm']=='stored_defaults' and r['phase']=='answer' and r.get('activePreferenceCount',0)>0 for r in rows)
    blocked = seen != expected or any(r['status'] != 'completed' for r in rows)
    return ('blocked' if blocked else 'completed'), summary


def run(split, live, arms):
    cases, truth = load_source(split)
    budget = len(cases) * sum(2 if arm in LEARNING_ARMS else 1 for arm in arms)
    if not live:
        print(json.dumps({'status': 'offline_only', 'split': split, 'cases': len(cases),
                          'maxProviderCalls': budget, 'modelScore': None}))
        return 0
    owned = Path(tempfile.mkdtemp(prefix='preference-eval-', dir=ROOT / 'logs'))
    report = {'status': 'running', 'split': split, 'arms': list(arms), 'sourceRevision': REVISION,
              'sourceManifestSha256': MANIFEST_SHA, 'checkedAt': datetime.now(timezone.utc).isoformat(),
              'maxProviderCalls': budget, 'results': [], 'limits': [
                  'Four frozen PrefEval MCQ cases; original inputs and deterministic option order, no inter-turn fillers',
                  'Memory-component adapter with existing automatic review, store and PromptBuilder; not full Main or tool acceptance',
                  'main_grounded separately uses production Main preparation/coordinator and original selected conversation; host rejects every tool request',
                  'main_memory closes/reopens the file DB and creates another project; actual Main receives stored preferences but no source conversation',
                  'stored_defaults forces one bounded review, ignores review cadence, then answers without raw history; pending habits not promoted',
                  'Choice accuracy is not free-generation or multi-day quality; learning costs count, no separate judge',
                  'Official CC BY-NC data stays in ignored cache; no ordinary data, configuration or desktop changes'],
              'sourceSha256': {p: digest(ROOT / p) for p in (
                  'scripts/preference_eval.py', 'src-tauri/src/agent/support/evolution.rs',
                  'src-tauri/src/agent/support/adaptive_constraints.rs', 'src-tauri/src/commands/memory.rs',
                  'src-tauri/src/agent/conversation/prompt.rs', 'src-tauri/src/commands/message_live_tests/preference_benchmark.rs',
                  'src-tauri/src/commands/message_live_tests/continuous_main.rs', 'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs')}}
    target = owned / 'acceptance.json'

    def record():
        report['usage'] = {key: sum(r.get(key, 0) for r in report['results'])
                           for key in ('requestCount', 'inputTokens', 'outputTokens')}
        report['usageIncomplete'] = report.get('usageIncomplete', False) or any(
            o.get('kind') == 'provider_error' for r in report['results'] for o in r.get('providerOutputs', []))
        target.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    print(f'PREFERENCE_EVAL_REPORT {target}', flush=True)
    record()
    try:
        for case in cases:
            for arm in arms:
                job = owned / f"{case['id']}-{arm}"
                job.mkdir()
                (job / 'input.json').write_text(json.dumps({'case': case, 'arm': arm}, ensure_ascii=False), encoding='utf-8')
                result, rows = run_context_worker(job, TEST, 'PREFERENCE_EVAL_ROW ')
                report['results'].extend(rows)
                report['status'], report['summary'] = score(cases, truth, report['results'], arms)
                record()
                if result.returncode or not rows or any(r['status'] != 'completed' for r in rows):
                    report['status'] = 'blocked'
                    return 2
                if report['usage']['inputTokens'] >= 120_000:
                    report['status'] = 'budget_exceeded'
                    return 2
        report['status'], report['summary'] = score(cases, truth, report['results'], arms)
    except BaseException:
        report['status'] = 'failed'
        report['usageIncomplete'] = True
        raise
    finally:
        record()
    print(json.dumps({'status': report['status'], 'summary': report['summary'], 'report': str(target)}))
    return 0 if report['status'] == 'completed' else 2


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--split', choices=('development', 'holdout'), default='development')
    parser.add_argument('--arm', choices=('all', *ARMS, 'main_grounded', 'main_memory'), default='all')
    parser.add_argument('--live', action='store_true')
    args = parser.parse_args()
    raise SystemExit(run(args.split, args.live, ARMS if args.arm == 'all' else (args.arm,)))
