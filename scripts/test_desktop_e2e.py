"""Offline regressions for canonical verification and isolated desktop artifacts."""

import json
import base64
import hashlib
import http.client
import contextlib
import copy
import io
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import sqlite3
import sys
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import desktop_e2e
import desktop_manual_acceptance as manual
import desktop_model_recovery as recovery
import desktop_project_restore as restore
import desktop_restore_assertions as restore_assertions
import browser_form_acceptance as browser_acceptance
import companion_eval
import agent_eval
import context_eval
import preference_eval
from prototypes import query_expansion_prototype as query_expansion
from prototypes import minimal_architecture
import native_uia_smoke as native
import verify


REPO_ROOT = desktop_e2e.ROOT


def memory_fixture_receipts(stages, paired=False):
    """Synthetic scoring receipts, not model or memory-capability evidence."""
    originals = [text for s in minimal_architecture.TRANSFER if s['id'] != 'exception' for text in s['sources']]
    saved = [dict(id=f'old-{i}', content=text, explicit=True, ready=True)
             for i, text in enumerate(originals + minimal_architecture.NOISE)]
    reviews, readers, ordered, case_index = [], [], [], 0
    for stage_index, stage in enumerate(stages):
        source_id = 'source-' + stage['id']
        saved.append(dict(id=source_id, content=stage['sources'][0], explicit=True, ready=True))
        thinking = dict(protocol='openai_responses', effort='low', budgetTokens=None)
        review = dict(id=stage['id'], phase='learn', status='completed', model='fixture-luna',
            requestCount=1, inputTokens=100, outputTokens=10, reportedUsageCount=1, latencyMs=100,
            onlyNewSources=True, cursorAdvanced=True, fixtureAdmitted=True, prefixSha256=f'{stage_index+1:064x}',
            newMemories=[dict(category='preference', content=stage['sources'][0], importance=5)],
            storedPreferences=copy.deepcopy(saved),
            providerInputs=[dict(messages=[dict(content='\n'.join(stage['sources']))], resolvedThinking=thinking)],
            providerOutputs=[{}])
        reviews.append(review)
        ordered.append(review)
        for case in stage['probes']:
            source = next((s for s in stages if s['id'] == case.get('source_stage')), None)
            result = '[habit:source-'+source['id']+'] '+source['sources'][0] if source else '未找到明确品牌记录'
            system = 'identity\n## 当前时间\n本机当前时间为 '+case['clock']+'。解释relative\n\npolicy'
            arms = (('low', 'medium') if case_index % 2 == 0 else ('medium', 'low')) if paired else ('current',)
            for arm in arms:
                effort = arm if paired else 'low'
                base = dict(messages=[dict(role='system', content=system), dict(role='user', content=case['question'])],
                    tools=[dict(name='recall_memories')],
                    resolvedThinking=dict(protocol='openai_responses', effort=effort, budgetTokens=None))
                second = copy.deepcopy(base)
                second['messages'].append(dict(role='tool', content=result))
                row = dict(id=case['id'], arm=arm, model='fixture-luna', status='completed',
                    stage=stage['id'],
                    coldOpened=True, historyMessages=0, archiveUnchanged=True, archiveSha256=review['prefixSha256'],
                    requestCount=2, reportedUsageCount=2, inputTokens=100, outputTokens=10, latencyMs=100,
                    visible=minimal_architecture.NOISE[:12], archiveRecords=len(saved), omitted=len(saved)-12,
                    clock=case['clock'], requestedEffort=effort, text=case['expected'],
                    providerInputs=[base, second], providerOutputs=[{}, dict(kind='text', text=case['expected'])],
                    steps=[dict(tool_name='recall_memories', success=True,
                                arguments=dict(query='品牌' if case.get('lookup_required') else '通勤'))])
                readers.append(row)
                ordered.append(row)
            case_index += 1
    return ordered if paired else reviews + readers


class DesktopE2eArtifactTests(unittest.TestCase):
    def test_natural_acceptance_keeps_exact_source_snapshot(self):
        with tempfile.TemporaryDirectory(prefix='angelbot-natural-snapshot-') as temporary:
            root=Path(temporary); (root/'logs').mkdir()
            names=('scripts/context_eval.py','src-tauri/src/commands/message.rs','src-tauri/src/commands/memory.rs',
                   'src-tauri/src/commands/message_live_tests/continuous_main.rs','src-tauri/src/commands/message_live_tests/agent_benchmarks.rs',
                   'src-tauri/src/agent/support/adaptive_constraints.rs','src-tauri/src/agent/conversation/prompt.rs',
                   'src-tauri/src/agent/support/evolution.rs','src-tauri/src/agent/extensions/memory.rs',
                   'src-tauri/src/commands/foreground_turn_coordinator.rs')
            for name in names:
                path=root/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(b'original source\n')
            def inspect(owned,*args,**kwargs):
                for name in names:self.assertEqual((owned/'source-snapshot'/name).read_bytes(),b'original source\n')
                (root/names[0]).write_bytes(b'changed during worker\n')
                return SimpleNamespace(returncode=0),[]
            with patch.object(context_eval,'ROOT',root),patch.object(context_eval,'run_context_worker',side_effect=inspect),contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(context_eval.run_natural_preferences(True),2)
            report=json.loads(next((root/'logs').glob('natural-preference-*/acceptance.json')).read_text(encoding='utf-8'))
            self.assertFalse(report['checks']['codeUnchanged'])
            self.assertEqual(report['status'],'needs_work')
            copy_file=shutil.copyfile
            def altered_copy(source,target):
                copy_file(source,target);Path(target).write_bytes(b'changed before worker\n')
            with patch.object(context_eval,'ROOT',root),patch.object(context_eval.shutil,'copyfile',side_effect=altered_copy), \
                 patch.object(context_eval,'run_context_worker') as worker,contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(ValueError,'Source changed before'):
                    context_eval.run_natural_preferences(True)
                worker.assert_not_called()

    def test_context_worker_keeps_streamed_receipts_on_timeout_and_preserves_returns(self):
        marker = 'CAPTURE_ROW '
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            script = root / 'probe.py'
            timed = root / 'timed'
            timed.mkdir()
            output = "print('CAPTURE_ROW {\"id\":\"partial\"}', flush=True)\n"
            error = "print('CAPTURE_ROW {\"id\":\"stderr-only\"}', file=sys.stderr, flush=True)\n"
            script.write_text('import sys, time\n' + output + error +
                              "sys.stdout.buffer.write(b'\\xe4\\xb8'); sys.stdout.buffer.flush()\n" +
                              'time.sleep(30)\n', encoding='utf-8')
            with patch.object(context_eval, 'build_cargo_binary', return_value=Path(sys.executable)), \
                 patch.object(context_eval, 'isolated_environment', return_value={}):
                with self.assertRaises(subprocess.TimeoutExpired) as interrupted:
                    context_eval.run_context_worker(timed, str(script), marker, timeout=2)
                raw = (timed / 'worker.stdout.log').read_bytes()
                self.assertIn(b'partial', raw)
                self.assertTrue(raw.endswith(b'\xe4\xb8'))
                self.assertEqual(interrupted.exception.output, raw)
                self.assertEqual(interrupted.exception.stderr, (timed / 'worker.stderr.log').read_bytes())
                self.assertIn('stderr-only', (timed / 'worker.stderr.log').read_text(encoding='utf-8'))
                combined = (timed / 'worker.log').read_text(encoding='utf-8')
                self.assertIn('partial', combined)
                self.assertIn('stderr-only', combined)
                self.assertIn('\ufffd', combined)
                log_failure = root / 'log-failure'
                log_failure.mkdir()
                write_text = Path.write_text

                def fail_log_write(path, *args, **kwargs):
                    if path == log_failure / 'worker.log':
                        raise OSError('synthetic preservation failure')
                    return write_text(path, *args, **kwargs)

                with patch.object(Path, 'write_text', new=fail_log_write):
                    with self.assertRaises(subprocess.TimeoutExpired) as failed_preservation:
                        context_eval.run_context_worker(log_failure, str(script), marker, timeout=2)
                self.assertIn('synthetic preservation failure', '\n'.join(failed_preservation.exception.__notes__))
                self.assertTrue((log_failure / 'worker.stdout.log').read_bytes().endswith(b'\xe4\xb8'))
                completed = root / 'completed'
                completed.mkdir()
                script.write_text('import sys\n' + output + error, encoding='utf-8')
                result, rows = context_eval.run_context_worker(completed, str(script), marker, timeout=10)
                self.assertEqual(result.returncode, 0)
                self.assertEqual(rows, [{'id': 'partial'}])
                self.assertEqual(result.stdout, (completed / 'worker.stdout.log').read_text(encoding='utf-8'))
                self.assertEqual(result.stderr, (completed / 'worker.stderr.log').read_text(encoding='utf-8'))
                self.assertEqual((completed / 'worker.log').read_text(encoding='utf-8'), result.stdout + '\n' + result.stderr)
                with patch.object(Path, 'write_text', new=fail_log_write):
                    with self.assertRaisesRegex(OSError, 'synthetic preservation failure'):
                        context_eval.run_context_worker(log_failure, str(script), marker, timeout=10)
                invalid = root / 'invalid'
                invalid.mkdir()
                script.write_text("import sys\nsys.stdout.buffer.write(b'\\xe4\\xb8')\n", encoding='utf-8')
                with self.assertRaises(UnicodeDecodeError):
                    context_eval.run_context_worker(invalid, str(script), marker, timeout=10)
                self.assertEqual((invalid / 'worker.stdout.log').read_bytes(), b'\xe4\xb8')

    def test_stability_manifest_precedes_worker_and_keeps_interruption_explicit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, owned = root / 'source', root / 'owned'
            source.mkdir()
            owned.mkdir()
            prefixes, hashes = {}, {}
            for prefix in (0, 1):
                filename = f'prefix-{prefix}.db'
                with contextlib.closing(sqlite3.connect(source / filename)) as conn:
                    conn.execute('PRAGMA user_version=1')
                hashes[filename] = hashlib.sha256((source / filename).read_bytes()).hexdigest()
                prefixes[prefix] = dict(file=filename, sha256=str(prefix + 1) * 64, sourceIds=[f'synthetic-{prefix}'])

            preservation = dict(inject=False, fail=False)

            def interrupted_worker(directory, test, marker, timeout):
                report = json.loads((directory / 'acceptance.json').read_text(encoding='utf-8'))
                self.assertEqual(report['status'], 'running')
                self.assertTrue(report['usageIncomplete'])
                self.assertEqual(report['sourceSha256'], hashes)
                self.assertEqual(len(report['codeSha256']), 9)
                self.assertIn('scripts/context_eval.py', report['codeSha256'])
                self.assertIn('src-tauri/src/agent/tools/registry.rs', report['codeSha256'])
                self.assertEqual(len(report['fixtureSha256']), 64)
                self.assertEqual(report['plan']['learningCalls'], 0)
                self.assertTrue((directory / 'input.json').is_file())
                preservation['fail'] = preservation['inject']
                raise subprocess.TimeoutExpired(['synthetic-worker'], 1)

            with patch.object(minimal_architecture, 'saved_prefix_source', return_value=(source, hashes, 'fixture-luna', prefixes)), \
                 patch.object(minimal_architecture.tempfile, 'mkdtemp', return_value=str(owned)), \
                 patch.object(minimal_architecture, 'run_context_worker', side_effect=interrupted_worker), \
                 contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(subprocess.TimeoutExpired):
                    minimal_architecture.run_stability(True, source)
            report = json.loads((owned / 'acceptance.json').read_text(encoding='utf-8'))
            self.assertEqual(report['status'], 'interrupted')
            self.assertEqual(report['interruptionType'], 'TimeoutExpired')
            self.assertTrue(report['usageIncomplete'])
            self.assertNotIn('results', report)
            self.assertFalse((owned / 'acceptance.tmp').exists())
            alias = root / 'alias'
            alias.mkdir()
            raw_failed_owned = alias / '..' / 'failed-preservation'
            raw_failed_owned.mkdir()
            # The worker canonicalizes mkdtemp, including Windows short aliases.
            failed_owned = raw_failed_owned.resolve()
            preservation['inject'] = True
            write_text = Path.write_text

            def fail_manifest_write(path, *args, **kwargs):
                if preservation['fail'] and path == failed_owned / 'acceptance.tmp':
                    raise OSError('synthetic manifest failure')
                return write_text(path, *args, **kwargs)

            with patch.object(minimal_architecture, 'saved_prefix_source', return_value=(source, hashes, 'fixture-luna', prefixes)), \
                 patch.object(minimal_architecture.tempfile, 'mkdtemp', return_value=str(raw_failed_owned)), \
                 patch.object(minimal_architecture, 'run_context_worker', side_effect=interrupted_worker), \
                 patch.object(Path, 'write_text', new=fail_manifest_write), contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(subprocess.TimeoutExpired) as interrupted:
                    minimal_architecture.run_stability(True, source)
            self.assertIn('OSError', '\n'.join(interrupted.exception.__notes__))
            pending = json.loads((failed_owned / 'acceptance.json').read_text(encoding='utf-8'))
            self.assertEqual(pending['status'], 'running')
            self.assertTrue(pending['usageIncomplete'])

    def test_minimal_architecture_keeps_gold_out_of_worker_and_requires_full_trace(self):
        payload = minimal_architecture.projection()
        self.assertEqual(len(payload['noise']), 36)
        self.assertEqual(len(set(payload['noise'])), 36)
        self.assertTrue(all(set(case) == {'id', 'question'} for case in payload['cases']))
        self.assertFalse(any(minimal_architecture.assess([])['checks'].values()))
        # A plausible sentence without actual learning/tool/source receipts fails.
        rows = [{'id': case['id'], 'arm': arm, 'text': case['expected'], 'status': 'completed'}
                for case in minimal_architecture.CASES for arm in minimal_architecture.ARMS]
        self.assertEqual(minimal_architecture.assess(rows)['status'], 'needs_work')
        self.assertFalse(minimal_architecture.assess(rows)['checks']['pairedActualInputs'])
        sequence = minimal_architecture.sequence_projection()
        self.assertEqual(sum(len(s['probes']) for s in sequence['stages']), 7)
        self.assertTrue(all(set(c) == {'id', 'question'} for s in sequence['stages'] for c in s['probes']))
        self.assertEqual(minimal_architecture.assess_sequence([])['status'], 'needs_work')
        self.assertEqual(minimal_architecture.run(False, True)['plan']['maxProviderCalls'], 46)
        self.assertEqual(minimal_architecture.assess_stale([], True)['status'], 'needs_work')
        self.assertEqual(minimal_architecture.assess_stale([], False)['status'], 'needs_work')
        view_cases=minimal_architecture.stale_cases('v5')
        self.assertFalse({c['id'] for c in view_cases} & {c['id'] for c in minimal_architecture.STALE_FOLLOWUP})
        self.assertEqual(minimal_architecture.assess_stale([], True, view_cases)['status'], 'needs_work')
        transfer = minimal_architecture.run(False, transfer=True)
        self.assertEqual(transfer['plan']['maxProviderCalls'], 25)
        self.assertEqual(transfer['input']['version'], 5)
        self.assertEqual(sum(len(s['sources']) for s in transfer['input']['stages']), 8)
        self.assertTrue(all(set(c) == {'id', 'question'} for s in transfer['input']['stages'] for c in s['probes']))
        self.assertFalse({c['id'] for c in transfer['cases']} & {c['id'] for c in view_cases})
        self.assertEqual(minimal_architecture.assess_transfer([])['status'], 'needs_work')
        plausible = [{'id': c['id'], 'text': c['expected'], 'arm': 'current', 'status': 'completed'} for c in transfer['cases']]
        self.assertEqual(minimal_architecture.assess_transfer(plausible)['status'], 'needs_work')
        # A complete receipt is accepted; dropping learning frames, source
        # readback or an explicit exclusion cannot pass just on answer text.
        receipts, saved = [], []
        for stage in minimal_architecture.TRANSFER:
            if stage['id'] != 'exception':
                saved.extend(dict(content=s, explicit=True, ready=True) for s in stage['sources'])
            receipts.append(dict(id=stage['id'], phase='learn', model='fixture-luna', status='completed',
                requestCount=1, inputTokens=1, outputTokens=0, onlyNewSources=True, cursorAdvanced=True,
                newCandidates=0 if stage['id'] == 'exception' else 2, preferenceStateUnchanged=stage['id'] == 'exception',
                storedPreferences=copy.deepcopy(saved),
                providerInputs=[dict(messages=[dict(content='\n'.join(stage['sources']))])], providerOutputs=[{}]))
        for case in transfer['cases']:
            result = '[habit:fixture] ' + case['readback'] if case.get('readback') else '未找到相关记忆'
            frames = [dict(messages=[dict(role='user', content=case['question'])], tools=[dict(name='recall_memories')]),
                      dict(messages=[dict(role='tool', content=result)], tools=[dict(name='recall_memories')])]
            receipts.append(dict(id=case['id'], arm='current', model='fixture-luna', status='completed',
                coldOpened=True, historyMessages=0, archiveUnchanged=True, requestCount=2, inputTokens=1,
                outputTokens=1, visible=minimal_architecture.NOISE[:12], archiveRecords=42, omitted=30,
                text=case['expected'], providerInputs=frames, providerOutputs=[{}, {}],
                steps=[dict(tool_name='recall_memories', success=True, arguments=dict(exclude_preferences=bool(case.get('exclude_preferences'))))]))
        self.assertEqual(minimal_architecture.assess_transfer(receipts)['status'], 'passed')
        broken = copy.deepcopy(receipts)
        broken[0]['providerInputs'] = []
        self.assertEqual(minimal_architecture.assess_transfer(broken)['status'], 'needs_work')
        broken = copy.deepcopy(receipts)
        broken[4]['providerInputs'][1]['messages'] = []
        self.assertEqual(minimal_architecture.assess_transfer(broken)['status'], 'needs_work')
        broken = copy.deepcopy(receipts)
        broken[-1]['steps'][0]['arguments']['exclude_preferences'] = False
        self.assertEqual(minimal_architecture.assess_transfer(broken)['status'], 'needs_work')
        natural = [dict(id='lookup_diagnostic', status='prepared', manualNoiseReordered=36,
            visible=minimal_architecture.NOISE[:12], omitted=30, oldResult='地铁 自行车', topicResult='地铁 自行车 打车')]
        for case, index in zip(minimal_architecture.NATURAL_CASES, (4, 5, 7)):
            row = copy.deepcopy(receipts[index])
            row['id'] = case['id']
            row['providerInputs'][0]['messages'][-1]['content'] = case['question']
            natural.append(row)
        self.assertEqual(minimal_architecture.assess_stale(natural, True, natural=True)['status'], 'passed')
        without_outputs = copy.deepcopy(natural)
        without_outputs[-1]['providerOutputs'] = []
        self.assertEqual(minimal_architecture.assess_stale(without_outputs, True, natural=True)['status'], 'needs_work')
        natural[-1]['providerInputs'][1]['messages'] = []
        self.assertEqual(minimal_architecture.assess_stale(natural, True, natural=True)['status'], 'needs_work')
        with self.assertRaises(ValueError):
            minimal_architecture.stale_source(REPO_ROOT)

    def test_temporal_memory_requires_actual_clock_originals_and_complete_readback(self):
        payload = minimal_architecture.temporal_projection('fixture-luna')
        self.assertEqual(payload['version'], 7)
        self.assertEqual(payload['noise'], [])
        followup = minimal_architecture.temporal_projection('fixture-luna', True)
        self.assertEqual(followup['version'], 8)
        self.assertEqual(sum(len(s['sources']) for s in followup['stages']), 3)
        self.assertTrue(all(set(c) == {'id', 'question', 'clock'} for s in followup['stages'] for c in s['probes']))
        self.assertFalse({c['id'] for s in followup['stages'] for c in s['probes']} &
                         {c['id'] for s in payload['stages'] for c in s['probes']})
        self.assertTrue(all(set(c) == {'id', 'question', 'clock'} for s in payload['stages'] for c in s['probes']))
        self.assertEqual(minimal_architecture.assess_temporal([])['status'], 'needs_work')
        rows = memory_fixture_receipts(minimal_architecture.TEMPORAL)
        rows[-2]['text'] += '。'  # Frozen protocol accepts a terminal full stop, not explanations.
        rows[-2]['providerOutputs'][-1]['text'] = rows[-2]['text']
        result = minimal_architecture.assess_temporal(rows)
        self.assertEqual(result['status'], 'passed')
        self.assertEqual(result['scores'], dict(current=6, exactOutput=5))
        for change in ('clock', 'original', 'readback', 'answer'):
            broken = copy.deepcopy(rows)
            if change == 'clock':
                broken[3]['providerInputs'][0]['messages'][0]['content'] += '\nextra policy'
            elif change == 'original':
                next(p for p in broken[0]['storedPreferences'] if p['id'] == 'source-future')['content'] = '公交车'
            elif change == 'readback':
                broken[4]['providerInputs'][1]['messages'][-1]['content'] = '没有实际回读'
            else:
                broken[2]['text'] = '公交车'
            self.assertEqual(minimal_architecture.assess_temporal(broken)['status'], 'needs_work', change)

    def test_budget_pair_requires_actual_effort_usage_prefix_and_both_arms(self):
        payload = minimal_architecture.budget_projection('fixture-luna')
        self.assertEqual(payload['version'], 9)
        self.assertTrue(all(set(c) == {'id', 'question', 'clock'} for s in payload['stages'] for c in s['probes']))
        rows = memory_fixture_receipts(minimal_architecture.BUDGET, paired=True)
        result = minimal_architecture.assess_budget(rows)
        self.assertEqual(result['status'], 'passed')
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['acceptanceScope'], 'medium_on_v9_fixture')
        self.assertEqual(result['scores'], {arm:dict(answerMatches=6, strictSuccess=6) for arm in ('low', 'medium')})
        self.assertFalse(result['costCandidate'])
        for fault in ('effort', 'usage', 'partner', 'prefix', 'question', 'admission', 'negative_lookup'):
            broken = copy.deepcopy(rows)
            reader = next(r for r in broken if r.get('arm') == 'medium')
            if fault == 'effort':
                for candidate in broken:
                    if candidate.get('arm') == 'medium':
                        for frame in candidate['providerInputs']:
                            frame['resolvedThinking']['effort'] = 'low'
            elif fault == 'usage':
                reader.pop('reportedUsageCount')
            elif fault == 'partner':
                broken.remove(reader)
            elif fault == 'prefix':
                reader['archiveSha256'] = '0'*64
            elif fault == 'question':
                reader['providerInputs'][0]['messages'][-1]['content'] += '偷换问题'
            elif fault == 'admission':
                broken[0]['fixtureAdmitted'] = False
            else:
                negative = next(r for r in broken if r.get('arm') == 'medium' and r['id'] == 'budget_unknown_brand')
                negative.update(requestCount=1, reportedUsageCount=1, steps=[],
                    providerInputs=negative['providerInputs'][:1], providerOutputs=negative['providerOutputs'][-1:])
            failed = minimal_architecture.assess_budget(broken)
            self.assertEqual(failed['status'], 'needs_work', fault)
            if fault != 'negative_lookup':
                self.assertFalse(failed['comparisonValid'], fault)
            else:
                self.assertTrue(failed['comparisonValid'])
            if fault == 'effort':
                self.assertFalse(failed['checks']['medium/actualEffort'])
                self.assertTrue(failed['checks']['medium/clockOnlyPair'])
            if fault == 'admission':
                self.assertTrue(failed['learningAdmissionFailure'])
        improved = copy.deepcopy(rows)
        low = next(r for r in improved if r.get('arm') == 'low')
        low['text'] = '未确认'
        low['providerOutputs'][-1]['text'] = '未确认'
        result = minimal_architecture.assess_budget(improved)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['status'], 'passed')
        self.assertEqual(result['scores']['low']['strictSuccess'], 5)
        self.assertEqual(result['scores']['medium']['strictSuccess'], 6)
        self.assertTrue(result['costCandidate'])
        next(r for r in improved if r.get('arm') == 'medium')['outputTokens'] = 10000
        result = minimal_architecture.assess_budget(improved)
        self.assertEqual(result['status'], 'passed')
        self.assertFalse(result['costCandidate'])

    def test_clock_view_reuses_exact_prefix_without_learning_or_changing_instant(self):
        source_rows = memory_fixture_receipts(minimal_architecture.BUDGET, paired=True)
        cases = minimal_architecture.BUDGET[0]['probes'][:2]
        rows = []
        for index, case in enumerate(cases):
            for arm in (('iso', 'local') if index == 0 else ('local', 'iso')):
                row = copy.deepcopy(next(r for r in source_rows if r.get('arm') == 'medium' and r['id'] == case['id']))
                rendered = case['clock'] if arm == 'iso' else minimal_architecture.local_clock(case['clock'])
                row.update(arm=arm, clock=rendered, sourceClock=case['clock'])
                for frame in row['providerInputs']:
                    frame['messages'][0]['content'] = frame['messages'][0]['content'].replace(case['clock'], rendered)
                rows.append(row)
        prefix = source_rows[0]['prefixSha256']
        assess = lambda rs: minimal_architecture.assess_clock_view(rs, 'fixture-luna', prefix, ['source-future'])
        result = assess(rows)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['scores'], {arm:dict(answerMatches=2, strictSuccess=2) for arm in ('iso', 'local')})
        self.assertEqual(result['learningUsage']['requestCount'], 0)
        self.assertFalse(result['candidateQualifiesForValidation'])
        class AsciiStrftimeDatetime(minimal_architecture.datetime):
            def strftime(self, fmt):
                # Python 3.11's Windows strftime uses the active ANSI locale.
                fmt.encode('ascii')
                return super().strftime(fmt)

        with patch.object(minimal_architecture, 'datetime', AsciiStrftimeDatetime):
            self.assertEqual(minimal_architecture.local_clock('2026-11-05T03:59:00-06:30'),
                             '2026年11月05日03时59分00秒（UTC-06:30）')
        for fault in ('partner', 'effort', 'usage', 'prefix', 'instant', 'hint', 'learning', 'readback'):
            broken = copy.deepcopy(rows)
            if fault == 'partner': broken.pop()
            elif fault == 'effort': broken[1]['providerInputs'][0]['resolvedThinking']['effort'] = 'low'
            elif fault == 'usage': broken[1]['reportedUsageCount'] = 1
            elif fault == 'prefix': broken[1]['archiveSha256'] = 'f' * 64
            elif fault == 'instant': broken[1]['providerInputs'][0]['messages'][0]['content'] = 'wrong instant'
            elif fault == 'hint': broken[1]['providerInputs'][0]['messages'][0]['content'] += '\nAlways use the older value.'
            elif fault == 'learning': broken.append(copy.deepcopy(source_rows[0]))
            else: broken[1]['providerInputs'][1]['messages'][-1]['content'] = 'no original source'
            failed = assess(broken)
            self.assertEqual(failed['status'], 'needs_work', fault)
            self.assertEqual(failed['comparisonValid'], fault == 'readback', fault)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'logs' / 'minimal-architecture-clock-source'
            source.mkdir(parents=True)
            snapshot = source / 'prefix-0.db'
            quote = minimal_architecture.BUDGET[0]['sources'][0]
            with contextlib.closing(sqlite3.connect(snapshot)) as conn:
                conn.execute('CREATE TABLE adaptive_constraints(id TEXT, constraint_value TEXT, source TEXT, status TEXT)')
                conn.execute('INSERT INTO adaptive_constraints VALUES(?,?,?,?)',
                             ('source-future', quote, 'explicit_user_preference', 'active'))
                conn.commit()
            source_rows[0]['prefixSnapshot'] = dict(file='prefix-0.db', sha256=hashlib.sha256(snapshot.read_bytes()).hexdigest())
            report = dict(experiment='budget_pair_v9', exitCode=0, comparisonValid=True,
                          sourceUnchanged=True, results=source_rows)
            (source / 'acceptance.json').write_text(json.dumps(report), encoding='utf-8')
            with patch.object(minimal_architecture, 'ROOT', root), patch.object(minimal_architecture, 'run_context_worker') as worker:
                plan = minimal_architecture.run_clock_view(False, source)
                self.assertEqual(plan['status'], 'offline_only')
                self.assertEqual(plan['plan']['learningCalls'], 0)
                self.assertTrue(all(set(c) == {'id', 'question', 'clock'} for c in plan['input']['cases']))
                worker.assert_not_called()
                snapshot.write_bytes(snapshot.read_bytes() + b'changed')
                with self.assertRaisesRegex(ValueError, 'checksum'):
                    minimal_architecture.run_clock_view(False, source)
                worker.assert_not_called()

    def test_stability_requires_two_identical_cold_trials_and_original_sources(self):
        template = next(r for r in memory_fixture_receipts(minimal_architecture.BUDGET, paired=True)
                        if r.get('arm') == 'medium')
        prefixes = {index:dict(sha256=str(index+1)*64, sourceIds=['source-'+stage['id']])
                    for index, stage in enumerate(minimal_architecture.BUDGET)}
        rows = []
        for trial in (1, 2):
            for case in (minimal_architecture.STABILITY if trial == 1 else minimal_architecture.STABILITY[::-1]):
                row = copy.deepcopy(template)
                first = copy.deepcopy(row['providerInputs'][0])
                first['messages'] = [dict(role='system', content='identity\n## 当前时间\n本机当前时间为 '+case['clock']+'。解释relative\n\npolicy'),
                                     dict(role='user', content=case['question'])]
                frames, steps = [first], []
                if 'source_prefix' in case or case.get('lookup_required'):
                    source = case.get('source_prefix')
                    result = ('[habit:'+prefixes[source]['sourceIds'][0]+'] '+minimal_architecture.BUDGET[source]['sources'][0]
                              if source is not None else '未找到明确品牌记录')
                    second = copy.deepcopy(first)
                    second['messages'].append(dict(role='tool', content=result))
                    frames.append(second)
                    steps.append(dict(tool_name='recall_memories', success=True,
                                      arguments=dict(query='品牌' if case.get('lookup_required') else '通勤')))
                row.update(id=case['id'], trial=trial, arm='stable', prefix=case['prefix'],
                    archiveSha256=prefixes[case['prefix']]['sha256'], clock=case['clock'], text=case['expected'],
                    requestCount=len(frames), reportedUsageCount=len(frames), providerInputs=frames,
                    providerOutputs=[{}]*(len(frames)-1)+[dict(kind='text', text=case['expected'])], steps=steps)
                rows.append(row)
        assess = lambda rs: minimal_architecture.assess_stability(rs, 'fixture-luna', prefixes)
        result = assess(rows)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['status'], 'passed')
        self.assertEqual(result['stability'], dict(cases=6, trials=2, allCasesBothPass=6, minTrialStrict=6,
                                                  answerInconsistency=0, strictInconsistency=0))
        self.assertEqual(result['learningUsage']['requestCount'], 0)
        self.assertTrue(all(not r['steps'] and r['requestCount'] == 1 for r in rows if r['id'] == 'stable_exception'))
        for fault in ('missing_trial', 'effort', 'usage', 'hint', 'prefix', 'historical_source', 'unknown_lookup'):
            broken = copy.deepcopy(rows)
            row = next(r for r in broken if r['trial'] == 2 and r['id'] == 'stable_history')
            if fault == 'missing_trial': broken.pop()
            elif fault == 'effort': row['providerInputs'][0]['resolvedThinking']['effort'] = 'low'
            elif fault == 'usage': row.pop('reportedUsageCount')
            elif fault == 'hint': row['providerInputs'][0]['messages'][0]['content'] += '\nUse the older default.'
            elif fault == 'prefix': row['archiveSha256'] = prefixes[0]['sha256']
            elif fault == 'historical_source':
                row['providerInputs'][-1]['messages'][-1]['content'] = '[habit:source-withdraw] 已撤回的旧计划'
            else:
                row = next(r for r in broken if r['trial'] == 2 and r['id'] == 'stable_unknown')
                row.update(steps=[], requestCount=1, reportedUsageCount=1,
                           providerInputs=row['providerInputs'][:1], providerOutputs=row['providerOutputs'][-1:])
            failed = assess(broken)
            self.assertEqual(failed['status'], 'needs_work', fault)
            self.assertEqual(failed['comparisonValid'], fault in ('historical_source', 'unknown_lookup'), fault)
        varied = copy.deepcopy(rows)
        one = next(r for r in varied if r['trial'] == 2 and r['id'] == 'stable_before')
        one['text'] = one['providerOutputs'][-1]['text'] = '骑自行车'
        self.assertEqual(assess(varied)['stability']['answerInconsistency'], 0)
        one['text'] = one['providerOutputs'][-1]['text'] = '未确认'
        failed = assess(varied)
        self.assertTrue(failed['comparisonValid'])
        self.assertEqual(failed['stability'], dict(cases=6, trials=2, allCasesBothPass=5, minTrialStrict=5,
                                                  answerInconsistency=1, strictInconsistency=1))
        for trial, word in ((1, '步行'), (2, '走路')):
            row = next(r for r in varied if r['trial'] == trial and r['id'] == 'stable_before')
            row['text'] = row['providerOutputs'][-1]['text'] = word
        wrong_but_consistent = assess(varied)
        self.assertEqual(wrong_but_consistent['stability']['answerInconsistency'], 0)
        self.assertEqual(wrong_but_consistent['stability']['allCasesBothPass'], 5)
        self.assertEqual(wrong_but_consistent['status'], 'needs_work')

    def test_existing_conversation_readers_require_received_verified_originals(self):
        template = next(r for r in memory_fixture_receipts(minimal_architecture.BUDGET, paired=True)
                        if r.get('arm') == 'medium')
        prefixes = {i:dict(sha256=str(i+1)*64, sourceIds=['source-'+stage['id']],
                          messageIds=[f'temporal-{i}-0'], quote=stage['sources'][0])
                    for i, stage in enumerate(minimal_architecture.BUDGET)}
        cases = minimal_architecture.STABILITY[2:4]
        rows = []
        for trial in (1, 2):
            for case in (cases if trial == 1 else cases[::-1]):
                proof = prefixes[case['source_prefix']]
                call_id = f'call-{trial}-{case["id"]}'
                args = dict(query='11月5日', limit=1)
                record = dict(message_id=proof['messageIds'][0], revision='a'*64,
                              role='user', text=proof['quote'])
                output = json.dumps(dict(historical_data=True, sources=[record]), ensure_ascii=False)
                first = copy.deepcopy(template['providerInputs'][0])
                first['messages'] = [dict(role='system', content='identity\n## 当前时间\n本机当前时间为 '+case['clock']+'。解释relative\n\npolicy'),
                                     dict(role='user', content=case['question'])]
                first['tools'] = [dict(name=name) for name in ('read_conversation', 'recall_conversation', 'recall_memories')]
                second = copy.deepcopy(first)
                second['messages'] += [dict(role='assistant', content='', toolCalls=[dict(id=call_id, name='recall_conversation', arguments=args)]),
                                       dict(role='tool', toolCallId=call_id, content='Result: '+output)]
                row = copy.deepcopy(template)
                row.update(id=case['id'], arm='stable', trial=trial, prefix=1, sourceReaders=True,
                           clock=case['clock'], archiveSha256=prefixes[1]['sha256'],
                           archiveRecords=44, omitted=32,
                           requestCount=2, reportedUsageCount=2, text=case['expected'],
                           providerInputs=[first, second], providerOutputs=[{}, dict(kind='text', text=case['expected'])],
                           steps=[dict(call_id=call_id, tool_name='recall_conversation', arguments=args, success=True, output=output)])
                rows.append(row)
        assess = lambda rs: minimal_architecture.assess_stability(rs, 'fixture-luna', prefixes, sources=True)
        result = assess(rows)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['status'], 'passed')
        self.assertEqual(result['stability']['allCasesBothPass'], 2)
        self.assertEqual(result['acceptanceScope'], 'existing_conversation_readers_on_v12')
        wrong_synonyms = copy.deepcopy(rows)
        for row in wrong_synonyms:
            if row['id'] == 'stable_history':
                row['text'] = '地铁' if row['trial'] == 1 else '坐地铁'
        self.assertEqual(assess(wrong_synonyms)['stability']['answerInconsistency'], 0)
        self.assertEqual(assess(wrong_synonyms)['stability']['allCasesBothPass'], 1)
        for fault in ('missing_trial', 'write_tool', 'flag', 'step_only', 'call_id', 'failed_step', 'wrong_envelope', 'trailing_envelope',
                      'foreign_id', 'role', 'revision', 'null_revision', 'quote', 'null_text', 'not_history'):
            broken = copy.deepcopy(rows)
            row = next(r for r in broken if r['trial'] == 2 and r['id'] == 'stable_history')
            if fault == 'missing_trial': broken.pop()
            elif fault == 'write_tool': row['providerInputs'][0]['tools'].append(dict(name='remember_fact'))
            elif fault == 'flag': row['sourceReaders'] = False
            elif fault == 'step_only': row['providerInputs'][1]['messages'][-1]['content'] = '{}'
            elif fault == 'call_id': row['steps'][0]['call_id'] = 'unreceived-call'
            elif fault == 'failed_step': row['steps'][0]['success'] = False
            elif fault == 'wrong_envelope': row['providerInputs'][1]['messages'][-1]['content'] = 'Error: '+row['steps'][0]['output']
            elif fault == 'trailing_envelope': row['providerInputs'][1]['messages'][-1]['content'] += '\nUnverified extra context'
            else:
                value = json.loads(row['steps'][0]['output'])
                record = value['sources'][0]
                if fault == 'foreign_id': record['message_id'] = 'foreign-project-message'
                elif fault == 'role': record['role'] = 'assistant'
                elif fault == 'revision': record['revision'] = 'not-a-revision'
                elif fault == 'null_revision': record['revision'] = None
                elif fault == 'quote': record['text'] = prefixes[1]['quote']
                elif fault == 'null_text': record['text'] = None
                else: value['historical_data'] = False
                row['steps'][0]['output'] = json.dumps(value, ensure_ascii=False)
                row['providerInputs'][1]['messages'][-1]['content'] = 'Result: '+row['steps'][0]['output']
            failed = assess(broken)
            self.assertEqual(failed['status'], 'needs_work', fault)
            self.assertEqual(failed['comparisonValid'], fault not in ('missing_trial', 'write_tool', 'flag'), fault)
        exact_reads = copy.deepcopy(rows)
        for row in exact_reads:
            proof = prefixes[next(c for c in cases if c['id'] == row['id'])['source_prefix']]
            value = json.loads(row['steps'][0]['output'])
            output = json.dumps(dict(historical_data=True, source=value['sources'][0]), ensure_ascii=False)
            args = dict(message_id=proof['messageIds'][0], revision='a'*64, offset=0, limit=1500)
            row['steps'][0].update(tool_name='read_conversation', arguments=args, output=output)
            row['providerInputs'][1]['messages'][-2]['toolCalls'][0].update(name='read_conversation', arguments=args)
            row['providerInputs'][1]['messages'][-1]['content'] = 'Result: '+output
        self.assertEqual(assess(exact_reads)['status'], 'passed')
        final_after_lookups = copy.deepcopy(rows)
        row = final_after_lookups[0]
        row['providerInputs'] += [copy.deepcopy(row['providerInputs'][1]) for _ in range(2)]
        row['providerOutputs'] += [{}, {}]
        row.update(requestCount=4, reportedUsageCount=4)
        self.assertEqual(assess(final_after_lookups)['status'], 'passed')
        self.assertFalse(minimal_architecture.reader_contract(dict(row, arm='current'), cases[0],
            ('recall_memories', 'recall_conversation', 'read_conversation'), max_calls=3))
        row['providerInputs'].append(copy.deepcopy(row['providerInputs'][1]))
        row['providerOutputs'].append({})
        row.update(requestCount=5, reportedUsageCount=5)
        self.assertFalse(assess(final_after_lookups)['comparisonValid'])

        conditional = []
        for trial in (1, 2):
            for case in (minimal_architecture.CONDITIONS if trial == 1 else minimal_architecture.CONDITIONS[::-1]):
                original = next(c for c in cases if c['source_prefix'] == case['source_prefix'])
                row = copy.deepcopy(next(r for r in rows if r['trial'] == trial and r['id'] == original['id']))
                row.update(id=case['id'], text=case['expected'])
                row['providerOutputs'][-1]['text'] = case['expected']
                for frame in row['providerInputs']:
                    frame['messages'][1]['content'] = case['question']
                conditional.append(row)
        assess_conditions = lambda rs: minimal_architecture.assess_stability(rs, 'fixture-luna', prefixes, True, 'conditions')
        result = assess_conditions(conditional)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['stability']['allCasesBothPass'], 3)
        self.assertEqual(result['sourceReadbacks'], 6)
        self.assertEqual(result['acceptanceScope'], 'explicit_conditions_on_v13')
        daily = []
        for trial in (1, 2):
            for case in (minimal_architecture.DAILY if trial == 1 else minimal_architecture.DAILY[::-1]):
                original = next(c for c in cases if c['source_prefix'] == case.get('source_prefix', 1))
                row = copy.deepcopy(next(r for r in rows if r['trial'] == trial and r['id'] == original['id']))
                answer = json.dumps({'方式':case['expected'],'说明':'合成的出门提醒'},ensure_ascii=False)
                row.update(id=case['id'],text=answer)
                row['providerOutputs'][-1]['text'] = answer
                for frame in row['providerInputs']:
                    frame['messages'][1]['content'] = case['question']
                daily.append(row)
        assess_daily = lambda rs: minimal_architecture.assess_stability(rs, 'fixture-luna', prefixes, True, 'daily')
        result = assess_daily(daily)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['stability']['allCasesBothPass'], 4)
        self.assertEqual(result['acceptanceScope'], 'natural_task_cards_on_v15')
        self.assertEqual(result['observed'], dict(expected=8,received=8,completed=8,answerMatches=8,strictSuccess=8,
                         pairedCases=4,pairedAnswerChanges=0,pairedStrictChanges=0))
        partial = assess_daily(daily[:3])
        self.assertFalse(partial['comparisonValid'])
        self.assertEqual(partial['observed'], dict(expected=8,received=3,completed=3,answerMatches=3,strictSuccess=3,
                         pairedCases=0,pairedAnswerChanges=0,pairedStrictChanges=0))
        unavailable = copy.deepcopy(daily)
        unavailable[-1]['status'] = 'unavailable'
        observed = assess_daily(unavailable)['observed']
        self.assertEqual(observed['completed'], 7)
        self.assertEqual(observed['pairedCases'], 3)
        self.assertEqual(observed['pairedAnswerChanges'], 0)
        different_inputs = copy.deepcopy(daily)
        different_inputs[0]['providerInputs'][0]['messages'][0]['content'] += '\nDifferent system'
        self.assertFalse(assess_daily(different_inputs)['comparisonValid'])
        self.assertEqual(assess_daily(different_inputs)['observed']['pairedCases'], 3)
        self.assertNotIn('daily_once/sourceReadback', result['outcomes']['1'])
        for fault in ('wrong_mode', 'empty_note', 'extra_field', 'bad_json'):
            broken = copy.deepcopy(daily)
            row = next(r for r in broken if r['id']=='daily_rain' and r['trial']==1)
            card = {'方式':'打车','说明':'出门带伞'}
            if fault == 'wrong_mode': card['方式'] = '自行车'
            elif fault == 'empty_note': card['说明'] = ''
            elif fault == 'extra_field': card['未请求字段'] = 'extra'
            row['text'] = '{' if fault == 'bad_json' else json.dumps(card,ensure_ascii=False)
            failed = assess_daily(broken)
            self.assertTrue(failed['comparisonValid'], fault)
            self.assertEqual(failed['status'], 'needs_work', fault)
            self.assertEqual(failed['outcomes']['1']['daily_rain/format'], fault == 'wrong_mode', fault)
        daily[0]['text'] = '```json\n'+daily[0]['text']+'\n```'
        self.assertEqual(assess_daily(daily)['status'], 'passed')
        with patch.object(minimal_architecture, 'saved_prefix_source', return_value=(Path('synthetic'), {}, 'fixture-luna', prefixes)):
            plan = minimal_architecture.run_stability(False, Path('unused'), True, 'daily')
        self.assertEqual(plan['input']['version'], 15)
        self.assertEqual(plan['plan']['readerDecisions'], 8)
        self.assertEqual(plan['plan']['maxProviderCalls'], 32)
        self.assertTrue(all(set(c)=={'id','question','clock','prefix'} for c in plan['input']['cases']))
        self.assertTrue(all(word not in c['question'] for c in minimal_architecture.DAILY[:2] for word in ('记忆','约定','习惯','查记录')))
        self.assertFalse(assess_conditions(conditional[:-1])['comparisonValid'])
        drift = copy.deepcopy(conditional)
        for row in drift:
            if row['id'] == 'condition_rain':
                row['providerInputs'][0]['messages'][0]['content'] += '\nDifferent guidance'
        self.assertTrue(assess_conditions(drift)['checks']['condition_rain/repeatInputIdentical'])
        self.assertFalse(assess_conditions(drift)['checks']['1/weatherOnlyInput'])
        # A fixed public inference frame is reconstructed without dispatching
        # tools. Only source ordering may differ, not wording or membership.
        frame = copy.deepcopy(conditional[0]['providerInputs'][1])
        output = json.loads(frame['messages'][-1]['content'][len('Result: '):])
        found = dict(output['sources'][0], matched=True)
        context = dict(found, message_id='context', text='背景原话', matched=False)
        output['sources'] = [context, found, dict(context, message_id='context-after')]
        frame['messages'][-1]['content'] = 'Result: '+json.dumps(output,ensure_ascii=False,separators=(',', ':'),sort_keys=True)
        a, b = minimal_architecture.order_frames(frame)
        self.assertEqual(a, frame)
        self.assertEqual(json.loads(b['messages'][-1]['content'][len('Result: '):])['sources'],
                         [found, context, dict(context, message_id='context-after')])
        self.assertEqual(a['messages'][:-1], b['messages'][:-1])
        self.assertEqual(a['tools'], b['tools'])
        ordered = []
        for i, arm in enumerate(('chronological', 'matched_first', 'matched_first', 'chronological')):
            ordered.append(dict(id=i,arm=arm,model='fixture-luna',status='completed',toolExecutions=0,
                requestCount=1,reportedUsageCount=1,inputTokens=12,outputTokens=1,latencyMs=1,
                providerInputs=[copy.deepcopy(b if arm=='matched_first' else a)],
                providerOutputs=[dict(kind='text',text='步行',stopReason='EndTurn',truncated=False)]))
        assess_order = lambda rs: minimal_architecture.assess_order(rs, 'fixture-luna', frame)
        self.assertEqual(assess_order(ordered)['status'], 'passed')
        for fault in ('missing', 'usage', 'dispatch', 'question', 'source', 'extra_request'):
            broken = copy.deepcopy(ordered)
            if fault == 'missing': broken.pop()
            elif fault == 'usage': broken[0]['reportedUsageCount'] = 0
            elif fault == 'dispatch': broken[0]['toolExecutions'] = 1
            elif fault == 'question': broken[1]['providerInputs'][0]['messages'][1]['content'] += 'new'
            elif fault == 'extra_request': broken[0]['requestCount'] = 2
            else:
                broken[1]['providerInputs'][0]['messages'][-1]['content'] += 'new'
            self.assertFalse(assess_order(broken)['comparisonValid'], fault)
        ordered[1]['providerOutputs'] = [dict(kind='tool_calls',count=1)]
        self.assertTrue(assess_order(ordered)['comparisonValid'])
        self.assertEqual(assess_order(ordered)['dispositions'][1]['kind'], 'continue_evidence')
        self.assertEqual(assess_order(ordered)['scores']['matched_first'], 1)
        self.assertEqual(assess_order(ordered)['status'], 'needs_work')
        conditional[2]['text'] = '自行车'
        self.assertEqual(assess_conditions(conditional)['status'], 'needs_work', 'rain must not inherit the dry-weather default')
        for sources, scenario in [(False, 'conditions'), (True, 'unknown')]:
            with self.assertRaises(ValueError):
                minimal_architecture.stability_cases(sources, scenario)

    def test_counterfactual_profiles_require_matching_stored_sources_and_equal_first_frames(self):
        m = minimal_architecture
        prefixes = {p:dict(sha256=str(p+1)*64,sourceIds=[],messageIds=[],caseProofs={}) for p in (0,1)}
        for p, metadata in prefixes.items():
            for source in m.COUNTER_SOURCES:
                source_id = f'{source["id"]}-{p}'
                metadata['sourceIds'].append(source_id)
                metadata['messageIds'].append(source['id'])
                metadata['caseProofs'][f'{source["id"]}_{p}'] = dict(sourceIds=[source_id],messageIds=[source['id']],quote=source['quotes'][p])
        template = next(r for r in memory_fixture_receipts(m.BUDGET,paired=True) if r.get('arm')=='medium')
        rows = []
        for trial in (1,2):
            for case in (m.COUNTER if trial==1 else m.COUNTER[::-1]):
                proof = m.stability_proof(prefixes,case)
                call_id = f'call-{trial}-{case["id"]}'
                args = dict(query='小雨' if 'rain' in case['id'] else '下午茶')
                output = json.dumps(dict(historical_data=True,sources=[dict(message_id=proof['messageIds'][0],revision='a'*64,role='user',text=proof['quote'])]),ensure_ascii=False)
                first = copy.deepcopy(template['providerInputs'][0])
                first.update(messages=[dict(role='system',content='identity\n## 当前时间\n本机当前时间为 '+case['clock']+'。解释relative\n\npolicy'),dict(role='user',content=case['question'])],
                             tools=[dict(name=n) for n in ('read_conversation','recall_conversation','recall_memories')])
                second = copy.deepcopy(first)
                second['messages'] += [dict(role='assistant',content='',toolCalls=[dict(id=call_id,name='recall_conversation',arguments=args)]),dict(role='tool',toolCallId=call_id,content='Result: '+output)]
                text = json.dumps({'选择':case['expected'],'说明':'合成建议'},ensure_ascii=False)
                row = copy.deepcopy(template)
                row.update(id=case['id'],arm='stable',trial=trial,prefix=case['prefix'],clock=case['clock'],sourceReaders=True,
                    archiveSha256=prefixes[case['prefix']]['sha256'],archiveRecords=46,omitted=34,text=text,requestCount=2,reportedUsageCount=2,
                    providerInputs=[first,second],providerOutputs=[{},dict(kind='text',text=text,stopReason='EndTurn',truncated=False)],
                    steps=[dict(call_id=call_id,tool_name='recall_conversation',arguments=args,success=True,output=output)])
                rows.append(row)
        assess = lambda rs: m.assess_stability(rs,'fixture-luna',prefixes,True,'counter')
        result = assess(rows)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['stability']['allCasesBothPass'],4)
        self.assertEqual(result['sourceReadbacks'],8)
        drift = copy.deepcopy(rows)
        drift[0]['providerInputs'][0]['messages'][0]['content'] += '\nDifferent profile header'
        self.assertFalse(assess(drift)['checks']['1/counter_rain/profileOnlyInput'])
        hinted = copy.deepcopy(rows)
        for row in hinted:
            row['providerInputs'][0]['messages'][0]['content'] += m.COUNTER_SOURCES[0]['quotes'][0]
        self.assertTrue(assess(hinted)['checks']['1/counter_rain/profileOnlyInput'])
        self.assertFalse(assess(hinted)['checks']['counterTargetsAbsent'])
        foreign_quote = copy.deepcopy(rows)
        foreign_quote[1]['steps'][0]['output'] = foreign_quote[0]['steps'][0]['output']
        foreign_quote[1]['providerInputs'][1]['messages'][-1]['content'] = 'Result: '+foreign_quote[0]['steps'][0]['output']
        self.assertFalse(assess(foreign_quote)['outcomes']['1']['counter_rain_1/sourceReadback'])
        task_view = copy.deepcopy(rows)
        for row in task_view:
            case = next(c for c in m.COUNTER if c['id']==row['id'])
            quote = m.stability_proof(prefixes,case)['quote']
            row['visible'][0] = quote
            first = row['providerInputs'][0]
            first['messages'][0]['content'] = first['messages'][0]['content'].replace('identity\n',
                'identity\n\n## Learned interaction defaults\nShared guidance stays fixed.\n- user-stated preference: '+
                json.dumps(quote,ensure_ascii=False)+' (global scope, confidence 100%)\n\n')
            row.update(taskView=True,steps=[],requestCount=1,reportedUsageCount=1,providerInputs=[first],providerOutputs=row['providerOutputs'][-1:])
        assess_view = lambda rs: m.assess_stability(rs,'fixture-luna',prefixes,True,'counter-view')
        viewed = assess_view(task_view)
        self.assertTrue(viewed['comparisonValid'])
        self.assertEqual(viewed['stability']['allCasesBothPass'],4)
        self.assertEqual(viewed['initialSourceEvidence'],8)
        self.assertEqual(viewed['sourceReadbacks'],0)
        hidden = copy.deepcopy(task_view)
        quote = m.stability_proof(prefixes,m.COUNTER[0])['quote']
        hidden[0]['visible'][0] = 'unrelated record'
        hidden[0]['providerInputs'][0]['messages'][0]['content'] = hidden[0]['providerInputs'][0]['messages'][0]['content'].replace(quote,'unrelated record')
        self.assertFalse(assess_view(hidden)['outcomes']['1']['counter_rain_0/sourceReadback'])
        leaked_policy = copy.deepcopy(task_view)
        leaked_policy[0]['providerInputs'][0]['messages'][0]['content'] += '\nDifferent policy outside preferences'
        self.assertFalse(assess_view(leaked_policy)['checks']['1/counter_rain/profileOnlyInput'])
        leaked_inside = copy.deepcopy(task_view)
        leaked_inside[0]['providerInputs'][0]['messages'][0]['content'] = leaked_inside[0]['providerInputs'][0]['messages'][0]['content'].replace(
            'Shared guidance stays fixed.', 'Different policy inside preferences.')
        self.assertFalse(assess_view(leaked_inside)['checks']['1/counter_rain/profileOnlyInput'])
        changed_scope = copy.deepcopy(task_view)
        changed_scope[0]['providerInputs'][0]['messages'][0]['content'] = changed_scope[0]['providerInputs'][0]['messages'][0]['content'].replace(
            '(global scope, confidence 100%)', '(project scope, confidence 100%)')
        self.assertFalse(assess_view(changed_scope)['checks']['1/counter_rain/profileOnlyInput'])
        wrong = copy.deepcopy(rows)
        wrong[1]['text'] = json.dumps({'选择':'打车','说明':'通用建议'},ensure_ascii=False)
        self.assertEqual(assess(wrong)['status'],'needs_work')
        with patch.object(m,'saved_prefix_source',return_value=(Path('synthetic'),{},'fixture-luna',prefixes)):
            plan = m.run_stability(False,Path('unused'),True,'counter')
        self.assertEqual(plan['input']['version'],16)
        self.assertEqual(plan['input']['counter_sources'],m.COUNTER_SOURCES)
        self.assertTrue(all(set(c)=={'id','question','clock','prefix'} for c in plan['input']['cases']))
        with tempfile.TemporaryDirectory() as directory:
            owned = Path(directory).resolve()
            (owned/'counter-prefixes.json').write_text(json.dumps(prefixes,ensure_ascii=False),encoding='utf-8')
            for p in (0,1):
                folder = owned/f'prefix-{p}'
                folder.mkdir()
                with contextlib.closing(sqlite3.connect(folder/'state.db')) as conn:
                    conn.executescript('CREATE TABLE messages(id TEXT,session_id TEXT,content TEXT,role TEXT,is_provisional INTEGER); CREATE TABLE adaptive_constraints(id TEXT,constraint_key TEXT,constraint_value TEXT,source TEXT,status TEXT); CREATE TABLE adaptive_constraint_evidence(constraint_key TEXT,session_id TEXT,source TEXT);')
                    conn.execute("INSERT INTO messages VALUES('seq-0-0','s','synthetic','user',0)")
                    for source in m.COUNTER_SOURCES:
                        key = f'{source["id"]}-{p}'
                        quote = source['quotes'][p]
                        conn.execute("INSERT INTO messages VALUES(?,'s',?,'user',0)",(source['id'],quote))
                        conn.execute("INSERT INTO adaptive_constraints VALUES(?,?,?,'explicit_user_preference','active')",(key,key,quote))
                        conn.execute("INSERT INTO adaptive_constraint_evidence VALUES(?,'s',?)",(key,'explicit_user:'+source['id']))
                    conn.commit()
            self.assertEqual(m.counter_prefixes(owned),prefixes)
            with contextlib.closing(sqlite3.connect(owned/'prefix-1'/'state.db')) as conn:
                conn.execute("UPDATE messages SET content='changed original' WHERE id='counter_tea'")
                conn.commit()
            with self.assertRaises(ValueError):
                m.counter_prefixes(owned)

    def test_task_window_pair_changes_only_known_filler_and_omission_count(self):
        m = minimal_architecture
        prefixes = {p:dict(sha256=str(p+1)*64,sourceIds=[],messageIds=[],caseProofs={}) for p in (0,1)}
        for p, metadata in prefixes.items():
            for source in m.WINDOW_SOURCES:
                sid = f'{source["id"]}-{p}'
                metadata['sourceIds'].append(sid)
                metadata['messageIds'].append(source['id'])
                metadata['caseProofs'][f'{source["id"]}_{p}'] = dict(sourceIds=[sid],messageIds=[source['id']],quote=source['quotes'][p])
        template = next(r for r in memory_fixture_receipts(m.BUDGET,paired=True) if r.get('arm')=='medium')
        rows = []
        for trial in (1,2):
            for case in (m.WINDOW_CASES if trial==1 else m.WINDOW_CASES[::-1]):
                index = m.WINDOW_CASES.index(case)
                slots = 2 if (index+trial)%2==0 else 12
                quote = m.stability_proof(prefixes,case)['quote']
                visible = [quote]+[f'其它背景{i}' for i in range(11)]
                visible = visible[:slots]
                system = 'identity\n\n## Learned interaction defaults\nShared policy remains fixed.\n'
                system += f'Working view: {slots} shown; {46-slots} additional eligible records omitted. Rest of the guidance.\n'
                system += '\n'.join('- user-stated preference: '+json.dumps(v,ensure_ascii=False)+' (global scope, confidence 100%)' for v in visible)
                system += '\n\n## 当前时间\n本机当前时间为 '+case['clock']+'。解释relative\n\npolicy'
                first = copy.deepcopy(template['providerInputs'][0])
                first.update(messages=[dict(role='system',content=system),dict(role='user',content=case['question'])],
                             tools=[dict(name=n) for n in ('read_conversation','recall_conversation','recall_memories')])
                text = json.dumps({'选择':case['expected'],'说明':'合成建议'},ensure_ascii=False)
                row = copy.deepcopy(template)
                row.update(id=case['id'],arm='stable',trial=trial,prefix=case['prefix'],clock=case['clock'],sourceReaders=True,taskView=True,viewSlots=slots,
                    visible=visible,omitted=46-slots,archiveRecords=46,archiveSha256=prefixes[case['prefix']]['sha256'],
                    text=text,requestCount=1,reportedUsageCount=1,inputTokens=100*slots,steps=[],providerInputs=[first],
                    providerOutputs=[dict(kind='text',text=text,stopReason='EndTurn',truncated=False)])
                rows.append(row)
        assess = lambda rs: m.assess_stability(rs,'fixture-luna',prefixes,True,'view-budget')
        result = assess(rows)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['pairedViews']['bothViewsPassed'],4)
        self.assertEqual(result['initialSourceEvidence'],8)
        self.assertNotIn('stability',result)
        self.assertEqual(result['scoresByView']['12']['strictSuccess'],4)
        self.assertEqual(result['scoresByView']['2']['strictSuccess'],4)
        self.assertAlmostEqual(result['inputReductionRatio'],5/6)
        changed = copy.deepcopy(rows)
        changed[0]['providerInputs'][0]['messages'][0]['content'] = changed[0]['providerInputs'][0]['messages'][0]['content'].replace('Shared policy remains fixed.','Altered policy.')
        self.assertFalse(assess(changed)['checks']['window_shirt_0/windowOnlyInput'])
        changed = copy.deepcopy(rows)
        changed[0]['providerInputs'][0]['messages'][0]['content'] = changed[0]['providerInputs'][0]['messages'][0]['content'].replace('34 additional','35 additional')
        self.assertFalse(assess(changed)['comparisonValid'])
        missing = assess(rows[:-1])
        self.assertFalse(missing['comparisonValid'])
        self.assertIsNone(missing['inputReductionRatio'])
        with patch.object(m,'saved_prefix_source',return_value=(Path('synthetic'),{},'fixture-luna',prefixes)):
            plan = m.run_stability(False,Path('unused'),True,'view-budget')
        self.assertEqual(plan['input']['version'],18)
        self.assertEqual(plan['input']['counter_sources'],m.WINDOW_SOURCES)
        self.assertTrue(all(set(c)=={'id','question','clock','prefix'} for c in plan['input']['cases']))
        frame = copy.deepcopy(rows[0]['providerInputs'][0])
        frame['messages'][0]['content'] = frame['messages'][0]['content'].replace('Shared policy remains fixed.',m.LOOKUP_CURRENT)
        a,b = m.lookup_frames(frame)
        self.assertEqual(a,frame)
        restored = copy.deepcopy(b)
        restored['messages'][0]['content'] = restored['messages'][0]['content'].replace(m.LOOKUP_AWARE,m.LOOKUP_CURRENT)
        self.assertEqual(restored,a,'Only completed-lookup guidance changes, not originals or tools')
        inference_rows = [dict(id=i,arm=name,model='fixture-luna',status='completed',toolExecutions=0,
            requestCount=1,reportedUsageCount=1,inputTokens=12,outputTokens=1,latencyMs=1,
            providerInputs=[copy.deepcopy(b if name=='lookup_aware' else a)],
            providerOutputs=[dict(kind='text',text=rows[0]['text'],stopReason='EndTurn',truncated=False)])
            for i,name in enumerate(('current_contract','lookup_aware','lookup_aware','current_contract'))]
        self.assertEqual(m.assess_order(inference_rows,'fixture-luna',frame,True)['status'],'passed')
        inference_rows[0]['providerOutputs'] = [dict(kind='tool_calls',count=1)]
        decision = m.assess_order(inference_rows,'fixture-luna',frame,True)
        self.assertTrue(decision['comparisonValid'])
        self.assertEqual(decision['dispositions'][0]['kind'],'continue_evidence')
        self.assertEqual(decision['status'],'needs_work','A dispatched lookup is not task completion')
        inference_rows[1]['toolExecutions'] = 1
        self.assertFalse(m.assess_order(inference_rows,'fixture-luna',frame,True)['comparisonValid'])
        with patch.object(m,'order_source',return_value=(Path('synthetic'),'a'*64,'fixture-luna',frame)):
            lookup_plan = m.run_order_view(False,Path('unused'),True)
        self.assertEqual(lookup_plan['input']['version'],19)
        self.assertEqual(set(lookup_plan['input']),{'version','live','model','frame'})

    def test_completed_lookup_guards_keep_needed_reads_and_exact_treatment(self):
        m = minimal_architecture
        prefixes = {p:dict(sha256=str(p+1)*64,sourceIds=[f'pref-{p}'],messageIds=[f'message-{p}'],quote=f'有来源的原话{p}') for p in (0,1)}
        template = next(r for r in memory_fixture_receipts(m.BUDGET,paired=True) if r.get('arm')=='medium')
        rows = []
        for trial in (1,2):
            for case in (m.GUARDS if trial==1 else m.GUARDS[::-1]):
                aware = (m.GUARDS.index(case)+trial)%2==0
                visible = [prefixes[0]['quote'],prefixes[1]['quote']]+[f'背景{i}' for i in range(10)]
                first = copy.deepcopy(template['providerInputs'][0])
                system = 'identity\n\n## Learned interaction defaults\n'+m.LOOKUP_CURRENT+'\n'+'\n'.join(visible)
                system += '\n\n## 当前时间\n本机当前时间为 '+case['clock']+'。解释relative\n\npolicy'
                first.update(messages=[dict(role='system',content=system),dict(role='user',content=case['question'])],
                             tools=[dict(name=n) for n in ('read_conversation','recall_conversation','recall_memories')])
                if aware:
                    first = m.lookup_frames(first)[1]
                frames, steps = [first],[]
                text = json.dumps({'选择':case['expected'],'说明':'合成依据'},ensure_ascii=False)
                outputs = [dict(kind='text',text=text,stopReason='EndTurn',truncated=False)]
                if case.get('lookup_required'):
                    output, call_id = 'No supported brand found in bounded lookup.','lookup-brand'
                    step = dict(tool_name='recall_memories',success=True,call_id=call_id,output=output,arguments={'query':'通勤 自行车 品牌'})
                    second = copy.deepcopy(first)
                    second['messages'] += [dict(role='assistant',toolCalls=[dict(id=call_id,name='recall_memories',arguments=step['arguments'])]),dict(role='tool',toolCallId=call_id,content='Result: '+output)]
                    frames.append(second)
                    steps.append(step)
                    outputs.insert(0,dict(kind='tool_calls',count=1))
                row = copy.deepcopy(template)
                row.update(id=case['id'],arm='stable',trial=trial,prefix=case['prefix'],clock=case['clock'],sourceReaders=True,taskView=True,lookupAware=aware,
                    visible=visible,omitted=32,archiveRecords=44,archiveSha256=prefixes[case['prefix']]['sha256'],text=text,
                    requestCount=len(frames),reportedUsageCount=len(frames),steps=steps,providerInputs=frames,providerOutputs=outputs)
                rows.append(row)
        assess = lambda rs: m.assess_stability(rs,'fixture-luna',prefixes,True,'lookup-guards')
        result = assess(rows)
        self.assertTrue(result['comparisonValid'])
        self.assertEqual(result['pairedContracts']['bothContractsPassed'],6)
        self.assertNotIn('stability',result)
        for name in ('current_contract','lookup_aware'):
            self.assertEqual(result['scoresByContract'][name]['strictSuccess'],6)
        history_search = copy.deepcopy(rows)
        row = next(r for r in history_search if r['id']=='guard_unknown' and r['lookupAware'])
        row['steps'][0]['tool_name'] = 'recall_conversation'
        row['providerInputs'][1]['messages'][-2]['toolCalls'][0]['name'] = 'recall_conversation'
        self.assertTrue(assess(history_search)['outcomes'][str(row['trial'])]['guard_unknown/lookup'])
        forged = copy.deepcopy(rows)
        row = next(r for r in forged if r['id']=='guard_unknown' and r['lookupAware'])
        row.update(requestCount=1,reportedUsageCount=1,providerInputs=row['providerInputs'][:1],providerOutputs=row['providerOutputs'][-1:])
        self.assertFalse(assess(forged)['outcomes'][str(row['trial'])]['guard_unknown/lookup'],'A success step without the actual received result is not a readback')
        wrong_once = copy.deepcopy(rows)
        row = next(r for r in wrong_once if r['id']=='guard_exception' and r['lookupAware'])
        row['text'] = json.dumps({'选择':'自行车','说明':'错误沿用默认'},ensure_ascii=False)
        self.assertEqual(assess(wrong_once)['status'],'needs_work')
        drift = copy.deepcopy(rows)
        next(r for r in drift if r['lookupAware'])['providerInputs'][0]['messages'][0]['content'] += '\nUnrelated policy change'
        self.assertFalse(assess(drift)['comparisonValid'])
        different_model = copy.deepcopy(rows)
        different_model[0]['model'] = 'different-model'
        self.assertFalse(assess(different_model)['comparisonValid'])
        self.assertEqual(assess(different_model)['matchedPairUsage']['cases'],[])
        missing = assess(rows[:-1])
        self.assertFalse(missing['comparisonValid'])
        self.assertIsNone(missing['inputReductionRatio'])
        partial = copy.deepcopy(rows)
        partial[-1].update(status='unavailable',reportedUsageCount=0,inputTokens=0,outputTokens=0,text=None,steps=[],providerOutputs=[dict(kind='provider_error')])
        interrupted = assess(partial)
        aware_usage = interrupted['scoresByContract']['lookup_aware']
        self.assertEqual(aware_usage['readers'],6)
        self.assertEqual(aware_usage['completedReaders'],5)
        self.assertEqual(aware_usage['unmeteredRequests'],1)
        self.assertFalse(aware_usage['usageComplete'])
        self.assertEqual(len(interrupted['matchedPairUsage']['cases']),5)
        self.assertNotIn('guard_before',interrupted['matchedPairUsage']['cases'])
        self.assertIsNone(interrupted['inputReductionRatio'])
        early = copy.deepcopy(rows)
        row = next(r for r in early if r['id']=='guard_before' and not r['lookupAware'])
        row['text'] = json.dumps({'选择':'步行','说明':'错误地提前采用未来约定'},ensure_ascii=False)
        self.assertTrue(assess(early)['comparisonValid'])
        self.assertFalse(assess(early)['outcomes'][str(row['trial'])]['guard_before/answer'])
        with patch.object(m,'saved_prefix_source',return_value=(Path('synthetic'),{},'fixture-luna',prefixes)):
            plan = m.run_stability(False,Path('unused'),True,'lookup-guards')
        self.assertEqual(plan['input']['version'],20)
        self.assertEqual(plan['plan']['readerDecisions'],12)
        self.assertEqual(plan['plan']['maxProviderCalls'],48)
        self.assertNotIn('counter_sources',plan['input'])
        self.assertTrue(all(set(c)=={'id','question','clock','prefix'} for c in plan['input']['cases']))

    def test_minimal_architecture_pair_normalization_keeps_non_preference_inputs(self):
        base = 'identity\n\n## Learned interaction defaults\n{}\n\n## 当前时间\nfixed\n\npolicy'
        self.assertEqual(minimal_architecture.without_preferences(base.format('full')),
                         minimal_architecture.without_preferences(base.format('bounded')))
        self.assertNotEqual(minimal_architecture.without_preferences(base.format('full')),
                            minimal_architecture.without_preferences(base.format('bounded').replace('fixed', 'other')))
    def test_query_expansion_prototype_counts_calls_without_promoting_generated_preferences(self):
        projected=query_expansion.projection(False)
        self.assertFalse(projected['live'])
        self.assertTrue(all(set(c)=={'id','query'} for c in projected['cases']))
        context_input=query_expansion.projection(False,True)
        self.assertTrue(all(set(c)<={'id','query','context'} for c in context_input['cases']))
        self.assertEqual(len(context_input['cases']),4)
        contextual=query_expansion.CONTEXT_CASES[1]
        self.assertEqual(json.loads(query_expansion.request_text(contextual)),{'query':'register','task_context':contextual['context']})
        self.assertTrue(all(r['content'] not in c.get('context','') for c in query_expansion.CONTEXT_CASES for r in query_expansion.CONTEXT_RECORDS))
        rows=[]
        records={r['key']:dict(r,id=r['key'],quote=r['content']) for r in query_expansion.RECORDS}
        for case in query_expansion.CASES:
            calls=int(case['id']!='exact_control')
            found=[records[k] for k in case['expected']]
            rows.append(dict(id=case['id'],query=case['query'],status='completed' if calls else 'skipped_hit',
                baseline=[] if calls else found,final=found,storeUnchanged=True,requestCount=calls,model='owned-luna',
                queries=['synthetic hint'] if calls else [],
                providerOutputs=[{'kind':'text','stopReason':'EndTurn','truncated':False,'text':'{"queries":["synthetic hint"]}'}] if calls else [],
                providerInputs=[{'tools':None,'messages':[{'role':'system','content':query_expansion.PROMPT},{'role':'user','content':case['query']}]}] if calls else []))
        self.assertEqual(query_expansion.assess(rows,True)[0],'passed')
        self.assertEqual(sum(r['requestCount'] for r in rows),4)
        for field,value in [('requestCount',2),('storeUnchanged',False),('status','invalid_expansion'),
                             ('model','different-provider'),('queries',['unrecorded hint']),('providerOutputs',[]),
                             ('final',[dict(records['drink'],quote='generated statement')]),
                             ('providerInputs',[{'tools':None,'messages':[{'role':'system','content':query_expansion.PROMPT},{'role':'user','content':query_expansion.RECORDS[0]['content']}]}])]:
            changed=json.loads(json.dumps(rows));changed[1][field]=value
            self.assertEqual(query_expansion.assess(changed,True)[0],'blocked',field)
        self.assertEqual(query_expansion.assess(rows[:-1],True)[0],'blocked','unrun cases are not passed')
        truncated=json.loads(json.dumps(rows));truncated[1]['providerOutputs'][0]['truncated']=True
        self.assertEqual(query_expansion.assess(truncated,True)[0],'blocked')
        wrong=json.loads(json.dumps(rows));wrong[1]['final']=[records['closing']]
        self.assertEqual(query_expansion.assess(wrong,True)[0],'needs_work','a real wrong source is not a transport failure')
        offline=json.loads(json.dumps(rows))
        for row in offline:
            row.update(status='skipped_hit' if row['baseline'] else 'would_expand',requestCount=0,providerInputs=[],providerOutputs=[],queries=[],final=row['baseline'])
        status,checks=query_expansion.assess(offline,False)
        self.assertEqual(status,'offline_only')
        self.assertTrue(all(c['expandedExact'] is None for c in checks.values()))

    def test_preference_benchmark_never_projects_rubrics_or_scores_incomplete_work_as_success(self):
        case=dict(id='prefeval.owned.explicit.0000',history=[{'role':'user','content':'Original preference'}],
                  question='Question',options=['a','b','c','d'],correct_label='B',preference_form='explicit')
        projected=preference_eval.project_case(case)
        self.assertEqual(set(projected),{'id','history','question','options'})
        self.assertNotIn('correct_label',json.dumps(projected))
        for role in ['system','tool']:
            with self.assertRaises(ValueError):
                preference_eval.project_case(dict(case,history=[{'role':role,'content':'Not a user'}]))
        self.assertEqual(preference_eval.choice('<choice>B</choice>'),'B')
        for text in ['B','<choice>A</choice><choice>B</choice>','<choice>E</choice>']:
            self.assertIsNone(preference_eval.choice(text))
        rows=[dict(id=case['id'],arm='stored_defaults',phase='learn',status='completed',requestCount=1,model='owned-luna'),
              dict(id=case['id'],arm='stored_defaults',phase='answer',status='completed',requestCount=1,model='owned-luna',
                   answer='<choice>B</choice>',toolCount=0,activePreferenceCount=1)]
        status,summary=preference_eval.score([projected],{case['id']:'B'},rows,('stored_defaults',))
        self.assertEqual(status,'completed')
        self.assertEqual(summary['stored_defaults']['correct'],1)
        self.assertEqual(summary['stored_defaults']['withDurablePreferences'],1)
        self.assertEqual(preference_eval.score([projected],{case['id']:'B'},rows[:1],('stored_defaults',))[0],'blocked')
        grounded=[dict(r,arm='main_grounded') for r in rows]
        with self.assertRaises(ValueError):
            preference_eval.score([projected],{case['id']:'B'},grounded,('main_grounded',))
        grounded[-1]['durableStatus']='completed'
        self.assertEqual(preference_eval.score([projected],{case['id']:'B'},grounded,('main_grounded',))[0],'completed')
        grounded[-1]['durableStatus']='failed'
        with self.assertRaises(ValueError):
            preference_eval.score([projected],{case['id']:'B'},grounded,('main_grounded',))
        memory=[dict(r,arm='main_memory') for r in rows]
        memory[-1].update(durableStatus='completed',sourceSession='source',session='consumer',
                          workingView={'originalMessages':0,'workingMessages':0})
        self.assertEqual(preference_eval.score([projected],{case['id']:'B'},memory,('main_memory',))[0],'completed')
        for field,value in [('sourceSession','consumer'),('session',None),
                            ('workingView',{'originalMessages':1,'workingMessages':0}),
                            ('workingView',{'originalMessages':0,'workingMessages':1})]:
            changed=[dict(r) for r in memory];changed[-1][field]=value
            with self.assertRaises(ValueError):
                preference_eval.score([projected],{case['id']:'B'},changed,('main_memory',))
        for field,value in [('requestCount',2),('toolCount',1),('model','different-luna')]:
            changed=[dict(r) for r in rows];changed[-1][field]=value
            with self.assertRaises(ValueError):
                preference_eval.score([projected],{case['id']:'B'},changed,('stored_defaults',))
        changed=[dict(r) for r in rows];changed[-1]['answer']='<choice>A</choice>'
        self.assertEqual(preference_eval.score([projected],{case['id']:'B'},changed,('stored_defaults',))[1]['stored_defaults']['correct'],0)
        with patch.object(preference_eval,'load_source',return_value=([projected],{case['id']:'B'})), \
                patch.object(preference_eval,'run_context_worker',side_effect=AssertionError('offline model call')), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(preference_eval.run('development',False,preference_eval.ARMS),0)
        self.assertIsNone(json.loads(output.getvalue())['modelScore'])

        failed=dict(rows[0],status='unavailable',inputTokens=0,outputTokens=0,
                    providerOutputs=[{'kind':'provider_error'}])
        with tempfile.TemporaryDirectory(prefix='angelbot-prefeval-report-') as owned, \
                patch.object(preference_eval,'load_source',return_value=([projected],{case['id']:'B'})), \
                patch.object(preference_eval.tempfile,'mkdtemp',return_value=owned), \
                patch.object(preference_eval,'run_context_worker',return_value=(SimpleNamespace(returncode=1),[failed])), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(preference_eval.run('development',True,('stored_defaults',)),2)
            report=json.loads((Path(owned)/'acceptance.json').read_text(encoding='utf-8'))
            self.assertTrue(report['usageIncomplete'],'unreported failed usage is not verified zero billing')
            self.assertEqual(report['summary']['stored_defaults']['planned'],1)

    def test_explicit_preference_score_requires_actual_learning_and_reopened_main(self):
        rows = [dict(id='learn', status='completed', requestCount=1, explicitReady=True,
                     providerInputs=[{'messages': [{'role': 'user', 'content': '原始偏好'}]}]),
                dict(id='respond', status='completed', requestCount=1, durableStatus='completed',
                     view={'workingMessages': 0, 'explicitPreferenceCount': 1}, toolExecutions=0,
                     sourceSession='source', session='other', answer='先挑一个最容易处理的小区域，整理五分钟即可。\n别急，慢慢来。',
                     providerInputs=[{'messages': [{'role': 'system', 'content': '已记录的偏好'}]}])]
        self.assertTrue(context_eval.score_explicit_preference(rows))
        self.assertTrue(all(context_eval.explicit_preference_checks(rows).values()))
        punctuated=[dict(r) for r in rows];punctuated[-1]['answer']='先整理一小块，别一次做太多。\n别急，慢慢来.'
        self.assertFalse(context_eval.explicit_preference_checks(punctuated)['exactTail'])
        self.assertFalse(context_eval.score_explicit_preference(punctuated),'do not regrade a punctuation failure as a strict pass')
        for index, field, value in [(0, 'explicitReady', False), (0, 'providerInputs', []),
                                    (1, 'requestCount', 2), (1, 'toolExecutions', 1),
                                    (1, 'durableStatus', 'failed'), (1, 'session', 'source'),
                                    (1, 'sourceSession', None), (1, 'answer', '别急，慢慢来。'),
                                    (1, 'providerInputs', [{'content': 'PRIVATE-PREFERENCE-SOURCE-7Q3'}]),
                                    (1, 'view', {'workingMessages': 1, 'explicitPreferenceCount': 1})]:
            with self.subTest(field=field, index=index):
                changed = [dict(row) for row in rows]
                changed[index][field] = value
                self.assertFalse(context_eval.score_explicit_preference(changed))
        self.assertFalse(context_eval.score_explicit_preference(rows[:1]))
        pressure = dict(id='respond', status='completed', durableStatus='completed', model='owned-luna',
                        requestCount=2, inputTokens=14000, outputTokens=20, sourceSession='source', session='other',
                        recordedUsage={'requestCount':2,'inputTokens':14000,'outputTokens':20,'models':['owned-luna','owned-luna']},
                        view={'originalMessages': 0, 'workingMessages': 0, 'explicitPreferenceCount': 12, 'preferenceOmitted':50},
                        toolExecutions=1, memoryReads=[{'toolName': 'recall_memories', 'success': True, 'targetSeen': True}],
                        recallPressure=True, scopeViolation=None, answer='<choice>A</choice>',
                        providerInputs=[{'messages': [{'role': 'system', 'content': '合成噪声偏好：50 additional eligible records omitted'}]},
                                        {'messages': [{'role': 'tool', 'content': '[habit:target] 末尾说别急，慢慢来。'}]}])
        learning = dict(model='owned-luna')
        self.assertTrue(all(context_eval.recall_pressure_checks(learning, pressure).values()))
        projected=copy.deepcopy(pressure)
        original='每个回答的末尾都另起一行写：别急，慢慢来。'
        actual_learning=dict(learning,storedPreferences=[dict(content=original,ready=True,explicit=True)])
        note={'selected_candidates':1,'scope':'global_explicit_preferences','complete_archive':False,'facts_searched':False}
        projected.update(requestCount=1,reportedUsageCount=1,toolExecutions=0,memoryReads=[],
            recordedUsage=dict(requestCount=1,inputTokens=14000,outputTokens=20,models=['owned-luna']))
        projected['view']=dict(projected['view'],taskPreferenceMatches=1)
        projected['providerInputs']=[{'messages':[{'role':'system','content':'50 additional eligible records omitted\nTask preference lookup: '+json.dumps(note,separators=(',',':'))+'\n'+json.dumps(original,ensure_ascii=False)}]}]
        self.assertTrue(all(context_eval.recall_pressure_checks(actual_learning,projected,task_preferences=True).values()))
        self.assertFalse(context_eval.recall_pressure_checks(actual_learning,projected)['outsideInitialProjection'],'Do not regrade old pressure results under the new delivery contract')
        for field,value in [('taskPreferenceMatches',0),('taskPreferenceMatches',3)]:
            broken=copy.deepcopy(projected);broken['view'][field]=value
            self.assertFalse(context_eval.recall_pressure_checks(actual_learning,broken,task_preferences=True)['taskProjectionDelivered'])
        for text in [json.dumps(note),json.dumps(original,ensure_ascii=False),
                'Task preference lookup: []\n'+json.dumps(original,ensure_ascii=False)]:
            broken=copy.deepcopy(projected);broken['providerInputs'][0]['messages'][0]['content']=text
            self.assertFalse(context_eval.recall_pressure_checks(actual_learning,broken,task_preferences=True)['taskProjectionDelivered'])
        for field, value in [
                ('answer', '<choice>D</choice>'), ('answer', '<choice>A</choice><choice>A</choice>'),
                ('answer', '<choice>A</choice> extra'),
                ('requestCount', 0), ('requestCount', 1), ('requestCount', 4), ('model', 'other-model'),
                ('memoryReads', []), ('memoryReads', [{'toolName': 'read_file', 'success': True, 'targetSeen': True}]),
                ('memoryReads', [{'toolName': 'recall_memories', 'success': False, 'targetSeen': True}]),
                ('memoryReads', [{'toolName': 'recall_memories', 'success': True, 'targetSeen': False}]),
                ('toolExecutions', 2), ('session', 'source'), ('sourceSession', None),
                ('status', 'needs_attention'), ('durableStatus', 'failed'), ('inputTokens', 0), ('inputTokens', 24001),
                ('recordedUsage',{}),
                ('recordedUsage',{'requestCount':1,'inputTokens':14000,'outputTokens':20,'models':['owned-luna']}),
                ('recordedUsage',{'requestCount':2,'inputTokens':14000,'outputTokens':20,'models':['other','other']}),
                ('recallPressure', False), ('scopeViolation', 'read_file rejected'),
                ('view', {'originalMessages': 1, 'workingMessages': 0, 'explicitPreferenceCount': 12}),
                ('view', {'originalMessages': 0, 'workingMessages': 1, 'explicitPreferenceCount': 12}),
                ('view', {'originalMessages': 0, 'workingMessages': 0, 'explicitPreferenceCount': 1}),
                ('view', {'originalMessages':0,'workingMessages':0,'explicitPreferenceCount':12,'preferenceOmitted':49}),
                ('providerInputs', []),
                ('providerInputs', [pressure['providerInputs'][0]]),
                ('providerInputs', [{'messages': [{'role': 'system', 'content': '别急，慢慢来'}]}, pressure['providerInputs'][1]]),
                ('providerInputs', [pressure['providerInputs'][0], {'messages': [{'role': 'user', 'content': '别急，慢慢来'}]}]),
                ('providerInputs', [{'messages':[{'role':'system','content':'合成噪声偏好'}]},pressure['providerInputs'][1]]),
                ('providerInputs', [pressure['providerInputs'][0], {'messages': [{'role': 'tool', 'content': '别急，慢慢来 PRIVATE-PREFERENCE-SOURCE-7Q3'}]}])]:
            with self.subTest(pressure_field=field, value=value):
                self.assertFalse(all(context_eval.recall_pressure_checks(learning, dict(pressure, **{field: value})).values()))
        with patch.object(context_eval, 'run_context_worker', side_effect=AssertionError('offline model call')), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(context_eval.run_explicit_preference(False), 0)
        self.assertIsNone(json.loads(output.getvalue())['modelScore'])

        variants=[context_eval.load_recall_variant(key) for key in ('closing_lookup','temporary_override')]
        self.assertEqual([v['answer'] for v in variants],['C','B'])
        with self.assertRaises(ValueError):context_eval.load_recall_variant('missing')
        for variant in variants:
            projected=context_eval.recall_variant_input(variant)
            self.assertEqual(set(projected),{'schemaVersion','question'})
            self.assertNotIn('"answer"',json.dumps(projected))
            answered=json.loads(json.dumps(pressure))
            answered['answer']=f"<choice>{variant['answer']}</choice>"
            answered['providerInputs'][0]['messages'].append({'role':'user','content':variant['question']})
            checks=context_eval.recall_pressure_checks(learning,answered,variant['answer'],variant['question'])
            self.assertTrue(all(checks.values()))
            self.assertFalse(context_eval.recall_pressure_checks(learning,answered,'A',variant['question'])['correctRecall'])
            self.assertFalse(context_eval.recall_pressure_checks(learning,answered,variant['answer'],'changed question')['frozenQuestion'])
        with self.assertRaises(ValueError):context_eval.run_explicit_preference(False,recall_variant='closing_lookup')

        with tempfile.TemporaryDirectory(prefix='angelbot-pref-reader-') as temporary:
            base=Path(temporary);source=base/'logs'/'explicit-preference-owned';source.mkdir(parents=True)
            recorded={'condition':'semantic_no_cue_v2','results':[dict(r,model='owned-luna') for r in rows]}
            target=source/'acceptance.json';target.write_text(json.dumps(recorded),encoding='utf-8')
            database=source/'main-data.db'
            with contextlib.closing(sqlite3.connect(database)) as conn, conn:
                conn.execute('CREATE TABLE projects(path TEXT,kind TEXT)')
                conn.execute('INSERT INTO projects VALUES(?,?)',(str(source/'workspace'),'project'))
            with patch.object(context_eval,'ROOT',base):
                owned,learned,hashes=context_eval.preference_learning_source(source)
                self.assertEqual(owned,source.resolve());self.assertEqual(learned['id'],'learn')
                self.assertEqual(set(hashes),{'acceptance.json','main-data.db'})
                for field,value in [('status','needs_attention'),('toolExecutions',1),('requestCount',2)]:
                    changed=json.loads(json.dumps(recorded));changed['results'][1][field]=value
                    target.write_text(json.dumps(changed),encoding='utf-8')
                    with self.assertRaises(ValueError):context_eval.preference_learning_source(source)
                target.write_text(json.dumps(recorded),encoding='utf-8')
                with contextlib.closing(sqlite3.connect(database)) as conn, conn:
                    conn.execute('UPDATE projects SET path=?',(str(base/'ordinary-user-data'),))
                with self.assertRaises(ValueError):context_eval.preference_learning_source(source)

    def test_main_journey_requires_process_checkpoint_and_physical_evidence(self):
        payload, stages, budget = context_eval.load_main()
        self.assertNotIn('expected', json.dumps(payload))
        self.assertEqual([s['phase'] for s in stages], [0, 0, 1, 1, 2, 2])
        self.assertEqual(budget['provider_calls'], 32)
        scope = json.loads((REPO_ROOT / 'scripts/fixtures/main_continuity_scope_v2.json').read_text(encoding='utf-8'))
        self.assertEqual(scope['base_fixture_sha256'], hashlib.sha256(context_eval.MAIN_FIXTURE.read_bytes()).hexdigest())
        self.assertEqual(set(scope['read_only_context']), {s['id'] for s in stages})
        self.assertFalse(scope['read_only_context']['agreement'])
        self.assertTrue(scope['read_only_context']['correction'])
        self.assertTrue(all(type(v) is bool for v in scope['read_only_context'].values()))
        rows = [dict(id=s['id'], phase=s['phase'], pid=100+s['phase'], session='same-main', status='completed',
                     requestCount=1, inputTokens=100, outputTokens=10, durableReply=True, scopeClean=True,
                     nativeCheckpoint=s['id']=='checkpoint', checkpointInput=s['id']=='resume', sourceView=s['id']=='checkpoint',
                     answer='ARCH-H32-Q9 草稿：收据缺项尚待确认，未获准发送。',
                     tools=([dict(toolName='write_file',success=True,output='saved')] if s['id'] in ('draft','next_task') else [])
                           + ([dict(toolName='read_file',success=True,output=s['expected']['readback'])] if s['expected'].get('readback') else [])) for s in stages]
        snapshot = {'draft.json': {'sha256':'same','valid':True}, 'draftUnchanged':True, 'sourcesIntact':True,
                    'costs.csv': {'sha256':'csv'}, 'archive.txt':{'sha256':'archive'}, 'check.txt':{'valid':True}}
        snapshots = [snapshot.copy() for _ in range(3)]
        for i,s in enumerate(snapshots):
            s['audit'] = {'writeAttempts':1 if i<2 else 2,'writeSuccesses':1 if i<2 else 2,'userTurns':2*(i+1),'assistantTurns':2*(i+1)}
        self.assertEqual(context_eval.score_main(stages, rows, snapshots)[0], 'passed')
        too_costly = [dict(r) for r in rows]
        too_costly[-1]['inputTokens'] = 240001
        status, cost_summary = context_eval.score_main(stages,too_costly,snapshots)
        self.assertEqual(status,'budget_exceeded','the final phase cannot bypass the resource target')
        self.assertEqual(cost_summary['passed'],6,'functional success must remain visible, not be erased by cost failure')
        self.assertFalse(cost_summary['withinResourceTarget'])
        light = [dict(r,workingView={'applied':True,'userOriginalsPreserved':True,'opaqueBefore':1,'opaqueAfter':0,'incompleteIndexes':0}) for r in rows]
        light[4]['checkpointInput'] = False
        self.assertEqual(context_eval.score_main(stages,light,snapshots)[0],'needs_work','the native-replay condition must not be silently weakened')
        light_status, light_summary = context_eval.score_main(stages,light,snapshots,'completed_view_prototype')
        self.assertEqual(light_status,'passed')
        self.assertFalse(light_summary['nativeRestoreVerified'])
        self.assertTrue(light_summary['completedViewRestored'])
        anchored = [dict(r,workingView=dict(r['workingView'],windowUserOriginalsPreserved=True,persistedSummary='stored checkpoint'),providerInputs=[{'messages':[{'role':'system','content':'stored checkpoint'}]}]) for r in light]
        anchored_status, anchored_summary = context_eval.score_main(stages,anchored,snapshots,'anchored_working_view')
        self.assertEqual(anchored_status,'passed')
        self.assertTrue(anchored_summary['persistedCheckpointRestored'])
        self.assertEqual(context_eval.score_main(stages,anchored,snapshots,'anchored_working_view_v2')[0],'passed')
        self.assertEqual(context_eval.score_main(stages,anchored,snapshots,'anchored_working_view_pressure8k')[0],'needs_work')
        pressure = [dict(r,workingView=dict(r['workingView'],historyBudgetTokens=8192,runnerWindowTokens=128000)) for r in anchored]
        self.assertEqual(context_eval.score_main(stages,pressure,snapshots,'anchored_working_view_pressure8k')[0],'passed')
        pressure[4]['workingView']['runnerWindowTokens']=8192
        self.assertEqual(context_eval.score_main(stages,pressure,snapshots,'anchored_working_view_pressure8k')[0],'needs_work')
        replay=dict(status='completed',requestCount=1,answer='杉影计划不对外发送；先说结果；先核对收据缺项再确认。ARCH-H32-Q9 user-ref archive-ref')
        self.assertTrue(context_eval.score_checkpoint_replay(replay,['user-ref','archive-ref']))
        for missing in ['杉影计划','收据缺项','确认','ARCH-H32-Q9','user-ref']:
            self.assertFalse(context_eval.score_checkpoint_replay(dict(replay,answer=replay['answer'].replace(missing,'')),['user-ref','archive-ref']))
        self.assertFalse(context_eval.score_checkpoint_replay(dict(replay,requestCount=2),['user-ref','archive-ref']))
        self.assertFalse(context_eval.score_checkpoint_replay(dict(replay,answer=replay['answer']+' r-'+'a'*22),['user-ref','archive-ref']))
        missing_wire = [dict(r) for r in anchored]; missing_wire[4]['providerInputs']=[]
        self.assertEqual(context_eval.score_main(stages,missing_wire,snapshots,'anchored_working_view')[0],'needs_work')
        self.assertEqual(context_eval.score_main(stages, rows[:-1], snapshots)[0], 'blocked')
        for field, index in [('nativeCheckpoint',3),('checkpointInput',4),('durableReply',4),('scopeClean',4)]:
            bad = [dict(r) for r in rows]; bad[index][field] = False
            self.assertEqual(context_eval.score_main(stages, bad, snapshots)[0], 'needs_work')
        bad = [dict(r,pid=100) for r in rows]
        self.assertEqual(context_eval.score_main(stages,bad,snapshots)[0], 'needs_work')
        bad = [dict(r) for r in rows]; bad[-1]['tools'] = rows[-1]['tools']+[dict(toolName='write_file',success=True,output='replayed')]
        self.assertEqual(context_eval.score_main(stages,bad,snapshots)[0], 'needs_work')
        with tempfile.TemporaryDirectory() as tmp:
            owned=Path(tmp); directory=owned/'workspace-A'; directory.mkdir()
            (directory/'draft.json').write_text('{"total":52,"status":"草稿"}',encoding='utf-8')
            original=context_eval.inspect_main_files(owned)['draft.json']['sha256']
            (directory/'draft.json').write_text('{"total":53,"status":"草稿"}',encoding='utf-8')
            changed=context_eval.inspect_main_files(owned,original)
            self.assertFalse(changed['draftUnchanged'])
            self.assertFalse(changed['draft.json']['valid'])

    def test_saved_source_neighborhood_is_an_opt_in_screen_not_a_new_gold_projection(self):
        from prototypes import preference_neighborhood as prototype
        payload, rubric = context_eval.load_variants('preference_neighborhood')
        self.assertEqual(len(payload), 4)
        self.assertEqual(context_eval.variant_call_budget(rubric), 8)
        self.assertTrue(all('expected' not in case for case in payload))
        for case in payload:
            for expanded in (False, True):
                view = prototype.view(case, expanded)
                self.assertLessEqual(len(view), 3)
                self.assertTrue(all('PRIVATE' not in item['quote'] and '这次' not in item['quote'] for item in view))
        answers = dict(drink_revision_v1='焙火乌龙', address_revision_v1='小岚',
                       one_turn_exception_v1='简短', source_isolation_v1='表格')
        rows = [dict(id=key[0],arm=key[1],turn=key[2],experiment='preference_neighborhood',
                     status='completed',requestCount=1,inputTokens=10,outputTokens=2,latencyMs=1,
                     answer=answers[key[0]]) for key in context_eval.variant_plan(rubric)]
        self.assertEqual(context_eval.score_variants(rubric, rows)[0], 'passed')
        rows[0]['answer'] = '绿茶'
        self.assertEqual(context_eval.score_variants(rubric, rows)[0], 'needs_work')
        self.assertEqual(context_eval.score_variants(rubric, rows[:-1])[0], 'blocked')
        # Existing fixed 48-call screen must remain unchanged.
        self.assertEqual(context_eval.variant_call_budget(context_eval.load_variants()[1]),48)
        grouped, rules=context_eval.load_variants('preference_grouping')
        self.assertEqual(len(grouped),4)
        self.assertEqual(context_eval.variant_call_budget(rules),8)
        self.assertFalse({c['id'] for c in payload}&{c['id'] for c in grouped})
        for case in grouped:
            self.assertNotIn('expected',case)
            self.assertEqual(prototype.episode_view(case)['episodes'][0]['statements'],prototype.view(case,True))
        records=prototype.view(grouped[0],True)
        envelope=lambda value:'data\n<saved-preference-candidates>'+json.dumps(value,ensure_ascii=False)+'</saved-preference-candidates>'
        flat=dict(sourceIds=[r['source'] for r in records],providerInputs=[{'messages':[
            {'role':'system','content':'fixed system'}, {'role':'user','content':envelope(records)},
            {'role':'user','content':'fixed question'}]}])
        episode=json.loads(json.dumps(flat))
        episode['providerInputs'][0]['messages'][1]['content']=envelope(prototype.episode_view(grouped[0]))
        self.assertTrue(context_eval.grouping_pair_contract(flat,episode))
        for index in (0,1,2):
            changed=json.loads(json.dumps(episode))
            changed['providerInputs'][0]['messages'][index]['content']+=' changed'
            self.assertFalse(context_eval.grouping_pair_contract(flat,changed))
        self.assertFalse(context_eval.grouping_pair_contract(flat,{}))
        with patch.object(sys,'argv',['context_eval.py','--suite','continuity','--experiment','preference_grouping','--arm','flat']), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as stopped:
                context_eval.main()
            self.assertEqual(stopped.exception.code,2)

    def test_continuity_screening_separates_gold_and_charges_all_turns(self):
        payload, rubric = context_eval.load_variants()
        self.assertEqual(len(payload), 8)
        self.assertEqual(context_eval.variant_call_budget(rubric), 48)
        subset, selected = context_eval.load_variants(case_ids=['draft_status', 'mail_tool_scope', 'notes_tool_scope'])
        self.assertEqual(len(subset), 3)
        self.assertEqual(context_eval.variant_call_budget(selected), 18)
        with self.assertRaises(ValueError):
            context_eval.load_variants(case_ids=['unknown'])
        for case in payload:
            self.assertNotIn('expected', case)
            self.assertNotIn('required', case)
            self.assertNotIn('padding', json.dumps(case))
        answers = {
            'draft_status': ['目前是草稿，未发送。'],
            'automation_status': ['本次失败，未生成新文件。'],
            'reading_update': ['每天20分钟，不主动推送提醒。'],
            'meeting_update': ['周四，邀请未发送。'],
            'habit_continuity': ['晚饭后读山居笔记20分钟。', '山居笔记。', '不会主动催你。'],
            'unfinished_continuity': ['草稿已准备，未发送。', '下一步核对报名信息。', '候选收件人林沐、主题秋日交流，你明确确认收件人和内容后才可以发送。'],
            'mail_tool_scope': ['这是草稿，未发送。'],
            'notes_tool_scope': ['未归档，还剩3项。'],
        }
        rows = [{'id': key[0], 'arm': key[1], 'turn': key[2], 'experiment': case['experiment'],
                 'status': 'completed', 'requestCount': 2 if case['experiment'] in {'result_projection', 'tool_selection'} else 1,
                 'inputTokens': 10, 'outputTokens': 2, 'latencyMs': 1, 'targetReads': 1,
                 'answer': answers[key[0]][key[2]-1] if key[2] else None}
                for key, (case, rule) in context_eval.variant_plan(rubric).items()]
        status, groups, amortization = context_eval.score_variants(rubric, rows)
        self.assertEqual(status, 'passed')
        self.assertEqual(groups['memory']['checkpoint']['inputTokens'], 80)
        self.assertEqual(groups['memory']['checkpoint']['buildInputTokens'], 20)
        self.assertEqual(amortization[2]['arms']['checkpoint']['tokens'], 96)
        self.assertEqual(amortization[2]['arms']['full']['tokens'], 72)
        changed = next(row for row in rows if row['id'] == 'habit_continuity' and row['arm'] == 'excerpt' and row['turn'] == 1)
        changed['answer'] = '每天45分钟，你想读哪本书？'
        self.assertEqual(context_eval.score_variants(rubric, rows)[0], 'needs_work')
        self.assertFalse(changed['factsMatched'])
        self.assertTrue(changed['reaskFlag'])
        changed.update(status='quota_exceeded', answer='山居笔记20分钟。')
        self.assertEqual(context_eval.score_variants(rubric, rows)[0], 'blocked')
        self.assertIsNone(changed['passed'])
        self.assertEqual(context_eval.score_variants(rubric, rows[:-1])[0], 'blocked')
        with self.assertRaises(ValueError):
            context_eval.score_variants(rubric, rows + [rows[0]])
        current = next(row for row in rows if row['experiment'] == 'neighborhood')
        current['requestCount'] = 0
        with self.assertRaises(ValueError):
            context_eval.score_variants(rubric, rows)

        production, rules = context_eval.load_variants('conversation_recall')
        self.assertEqual(len(production), 2)
        self.assertEqual(context_eval.variant_call_budget(rules), 8)
        self.assertTrue(all('expected' not in case for case in production))
        self.assertGreater(len(production[1]['history'][0]['content']), 2000)
        rows = [dict(id=case['id'], experiment='conversation_recall', arm='scoped', turn=1,
                     status='completed', requestCount=2, inputTokens=10, outputTokens=3, latencyMs=1,
                     targetReads=1, answer=answer)
                for case, answer in zip(rules, ['瓦尔登湖，每晚二十分钟，不主动提醒。',
                    '许棠，冬季读书会；草稿未核对、未发送；需你确认收件人和正文。'])]
        self.assertEqual(context_eval.score_variants(rules, rows)[0], 'passed')
        rows[0]['targetReads'] = 0
        self.assertEqual(context_eval.score_variants(rules, rows)[0], 'needs_work', 'answer-only guesses cannot pass production tool acceptance')

        receipts, rules = context_eval.load_variants('receipt_readback')
        self.assertEqual(len(receipts), 2)
        self.assertEqual(context_eval.variant_call_budget(rules), 16)
        self.assertTrue(all('expected' not in case for case in receipts))
        self.assertGreater(len(receipts[0]['receipt']['diagnostics']), 6000)
        self.assertGreater(len(json.dumps(receipts[1]['receipt']['instructions'])), 6000)
        answers = ['任务失败，HTTP 403拒绝访问，没有新文件。',
                   '周岚、周末活动报名；草稿未发送，需要你明确确认收件人和正文。']
        rows = [dict(id=case['id'], experiment='receipt_readback', arm=arm, turn=1,
                     status='completed', requestCount=3, inputTokens=10, outputTokens=2, latencyMs=1,
                     targetReads=1, sourceReads=1, answer=answer)
                for case, answer in zip(rules, answers) for arm in ('raw', 'envelope')]
        self.assertEqual(context_eval.score_variants(rules, rows)[0], 'passed')
        rows[-1]['sourceReads'] = 0
        self.assertEqual(context_eval.score_variants(rules, rows)[0], 'needs_work', 'omitted necessary facts must be read, not guessed')
        batched, batching_rules = context_eval.load_variants('receipt_batching')
        self.assertEqual(context_eval.variant_call_budget(batching_rules), 16)
        self.assertTrue(all(case['experiment'] == 'receipt_batching' for case in batched))
        self.assertEqual([case['receipt'] for case in receipts], [case['receipt'] for case in batched])
        self.assertEqual([case['questions'] for case in receipts], [case['questions'] for case in batched])
        self.assertEqual([case['expected'] for case in rules], [case['expected'] for case in batching_rules])
        guided, guidance_rules = context_eval.load_variants('receipt_guidance')
        self.assertEqual(context_eval.variant_call_budget(guidance_rules), 32)
        self.assertTrue({case['id'] for case in guided}.isdisjoint({case['id'] for case in receipts}))
        self.assertTrue(all('expected' not in case for case in guided))
        self.assertEqual(len(set(guided[0]['receipt']['diagnostics'].splitlines())), 160)
        answers = ['周六09:30；草稿尚未安排，需你确认时间和提醒内容后添加。',
                   '导出失败，没有新文件，目标文件被其他程序占用。']
        guided_rows = [dict(id=case['id'], experiment='receipt_guidance', arm=arm, turn=1,
                           status='completed', requestCount=3, inputTokens=10, outputTokens=2, latencyMs=1,
                           targetReads=1, sourceReads=1, answer=answer)
                      for case, answer in zip(guidance_rules, answers)
                      for arm in context_eval.VARIANT_ARMS['receipt_guidance']]
        self.assertEqual(context_eval.score_variants(guidance_rules, guided_rows)[0], 'passed')
        guided_rows[0]['sourceReads'] = 0
        self.assertEqual(context_eval.score_variants(guidance_rules, guided_rows)[0], 'needs_work')
        budgeted, budgeted_rules = context_eval.load_variants('receipt_budget_catalog', ['guidance_export_v1'])
        self.assertEqual(context_eval.variant_call_budget(budgeted_rules), 8)
        self.assertEqual(budgeted[0]['receipt'], guided[1]['receipt'])
        self.assertEqual(budgeted_rules[0]['expected'], guidance_rules[1]['expected'])
        durable, durable_rules = context_eval.load_variants('durable_sources')
        self.assertEqual(context_eval.variant_call_budget(durable_rules), 16)
        self.assertNotIn('expected', durable[0])
        self.assertGreater(len(durable[0]['receipt']['details']['debug']), 6000)
        answers = ['归档失败，没有生成新文件，索引文件被锁定。',
                   '凭证ARCH-R47-268。用户确认占用解除后，重新授权归档。']
        durable_rows = [dict(id=durable[0]['id'], experiment='durable_sources', arm=arm, turn=turn,
                             status='completed', requestCount=3, inputTokens=10, outputTokens=2, latencyMs=1,
                             targetReads=1, sourceReads=1, inspectionExecutions=1, coldReopen=turn > 1, answer=text)
                        for arm in ('raw', 'compact') for turn, text in enumerate(answers, 1)]
        self.assertEqual(context_eval.score_variants(durable_rules, durable_rows)[0], 'passed')
        selected_rules = [dict(case, selected_arms=('compact',)) for case in durable_rules]
        selected_rows = [row for row in durable_rows if row['arm'] == 'compact']
        self.assertEqual(context_eval.variant_call_budget(selected_rules), 8)
        self.assertEqual(context_eval.score_variants(selected_rules, selected_rows)[0], 'passed')
        self.assertEqual(context_eval.score_variants(durable_rules, selected_rows)[0], 'blocked', 'a selected-condition pass cannot fill the original missing matrix')
        durable_rows[-1]['inspectionExecutions'] = 2
        self.assertEqual(context_eval.score_variants(durable_rules, durable_rows)[0], 'needs_work', 'correct text cannot excuse source-operation replay')

    def test_context_ablation_separates_gold_and_counts_checkpoint_generation(self):
        source = [{'question_id': key, 'question': 'q', 'question_date': '2023/01/03', 'question_type': 'synthetic',
                   'answer': 'gold-hidden', 'answer_session_ids': ['hidden'], 'haystack_dates': ['2023/01/02', '2023/01/01'],
                   'haystack_sessions': [[{'role': 'user', 'content': 'new', 'has_answer': True}], [{'role': 'user', 'content': 'old', 'has_answer': False}]]}
                  for key in context_eval.IDS]
        payload, rubric = context_eval.project_cases(source)
        self.assertNotIn('gold-hidden', json.dumps(payload))
        self.assertNotIn('has_answer', json.dumps(payload))
        self.assertEqual(set(payload[0]), {'id', 'question', 'question_date', 'history'})
        self.assertTrue(payload[0]['history'][0]['content'].endswith('old'))
        rows = [{'id': case['id'], 'arm': arm, 'status': 'completed', 'requestCount': 1,
                 'inputTokens': 10, 'outputTokens': 2, 'answer': case['expected'][0]}
                for case in rubric for arm in (*context_eval.ARMS, 'checkpoint_build')]
        status, groups = context_eval.score(rubric, rows)
        self.assertEqual(status, 'passed')
        self.assertEqual(groups['checkpoint']['includingBuildInputTokens'], 80)
        self.assertEqual(groups['checkpoint_build']['evaluated'], 0, 'generating a summary is not answering correctly')
        unknown = next(row for row in rows if row['id'].endswith('_abs') and row['arm'] == 'clean_full')
        unknown.update(status='unavailable', answer='UNKNOWN')
        self.assertEqual(context_eval.score(rubric, rows)[0], 'blocked')
        self.assertIsNone(unknown['passed'], 'provider failure cannot earn abstention credit')
        self.assertEqual(context_eval.score(rubric, rows[:-1])[0], 'blocked')
        with self.assertRaises(ValueError):
            context_eval.score(rubric, rows + [rows[0]])
        checkpoint = [row for row in rows if row['arm'] in {'checkpoint', 'checkpoint_build'}]
        status, groups = context_eval.score(rubric, checkpoint, ('checkpoint',))
        self.assertEqual(status, 'passed')
        self.assertEqual(groups['checkpoint']['includingBuildInputTokens'], 80)
        self.assertEqual(groups['noisy_full']['planned'], 0, 'unrun arms must not appear evaluated')
        self.assertEqual(context_eval.score(rubric, checkpoint[:-1], ('checkpoint',))[0], 'blocked')
        with self.assertRaises(ValueError):
            context_eval.score(rubric, rows, ('checkpoint',))

    def test_agent_benchmark_grader_rejects_failed_requests_and_unexecuted_claims(self):
        answers = {'simple_python_5': {'ground_truth': [{'solve_quadratic': {'a': [3], 'root_type': ['all']}}]}}
        row = {'id': 'simple_python_5', 'status': 'completed', 'calls': [{'name': 'solve_quadratic', 'arguments': {'a': 3}}], 'text': None}
        self.assertFalse(agent_eval.grade_call(row, answers), 'required semantics cannot be silently defaulted')
        row['calls'][0]['arguments']['root_type'] = 'all'
        self.assertTrue(agent_eval.grade_call(row, answers))
        self.assertFalse(agent_eval.equal(True, 1))
        irrelevant = {'id': 'irrelevance_0', 'status': 'unavailable', 'calls': [], 'text': None}
        self.assertIsNone(agent_eval.grade_call(irrelevant, answers))
        irrelevant.update(status='completed', text='No suitable function exists.')
        self.assertTrue(agent_eval.grade_call(irrelevant, answers))
        model_log = f'test {agent_eval.TEST} ... AGENT_EVAL_ROW ' + json.dumps(irrelevant)
        self.assertEqual(companion_eval.parse_rows(model_log, marker='AGENT_EVAL_ROW ', test=agent_eval.TEST), [irrelevant])
        task = {'id': 'delivery_change', 'status': 'completed', 'world': {'day': 'Thursday', 'revision': 18, 'writes': 1, 'reads_before': 1, 'reads_after': 1}}
        self.assertTrue(agent_eval.grade_state(task))
        task['world']['day'] = 'Wednesday'
        self.assertFalse(agent_eval.grade_state(task))
        task['world']['day'] = 'Thursday'
        task['world']['writes'] = 0
        self.assertFalse(agent_eval.grade_state(task), 'a completion claim is not a completed action')
        task['world']['writes'] = 2
        self.assertFalse(agent_eval.grade_state(task), 'duplicate writes are not a clean success')
        memory = {'id': 'memory_keyword', 'status': 'completed', 'text': '白毫银针', 'world': {'target_seen': True, 'irrelevant_seen': False}}
        self.assertTrue(agent_eval.grade_state(memory))
        memory['world']['target_seen'] = False
        self.assertFalse(agent_eval.grade_state(memory), 'guessed answers are not grounded recall')
        memory['world']['target_seen'] = True
        memory['text'] = '不是白毫银针'
        self.assertFalse(agent_eval.grade_state(memory))
        memory['text'] = '白毫银针'
        memory['world']['irrelevant_seen'] = True
        self.assertFalse(agent_eval.grade_state(memory), 'unrelated memory access violates the task')
        changed = agent_eval.schema({'type': 'dict', 'properties': {'numbers': {'type': 'array', 'items': {'type': 'float'}}}, 'required': []})
        self.assertEqual(changed['type'], 'object')
        self.assertEqual(changed['properties']['numbers']['items']['type'], 'number')
        self.assertEqual(changed['required'], [], 'optional parameters must remain optional')

    def test_agent_holdout_unordered_calls_and_nested_arguments(self):
        answers = {'parallel_1': {'ground_truth': [{'f': {'x': [1]}}, {'f': {'x': [2]}}]},
                   'multiple_9': {'ground_truth': [{'average': {'grades': [{'math': [90], 'music': [89]}]}}]}}
        row = {'id': 'parallel_1', 'status': 'completed', 'calls': [{'name': 'f', 'arguments': {'x': 2}}, {'name': 'f', 'arguments': {'x': 1}}]}
        self.assertTrue(agent_eval.grade_call(row, answers))
        row['calls'][1]['arguments']['x'] = 2
        self.assertFalse(agent_eval.grade_call(row, answers), 'one actual call cannot cover both references')
        row['calls'].pop()
        self.assertFalse(agent_eval.grade_call(row, answers), 'missing parallel calls cannot pass')
        nested = {'id': 'multiple_9', 'status': 'completed', 'calls': [{'name': 'average', 'arguments': {'grades': {'math': 90, 'music': 89}}}]}
        self.assertTrue(agent_eval.grade_call(nested, answers))
        nested['calls'][0]['arguments']['grades']['music'] = True
        self.assertFalse(agent_eval.grade_call(nested, answers))
        nested['calls'][0]['arguments']['grades']['music'] = 89
        nested['calls'][0]['arguments']['grades']['extra'] = 50
        self.assertFalse(agent_eval.grade_call(nested, answers))
        nested['calls'][0]['arguments']['grades'].pop('extra')
        nested['argumentsValid'] = False
        self.assertFalse(agent_eval.grade_call(nested, answers), 'host schema rejection cannot be a correct call')
        conflict = {'id': 'delivery_conflict', 'status': 'completed', 'world': {'day': 'Thursday', 'revision': 19, 'writes': 1, 'reads_before': 2, 'reads_after': 1}}
        self.assertTrue(agent_eval.grade_state(conflict))
        conflict['world']['revision'] = 18
        self.assertFalse(agent_eval.grade_state(conflict))
        updated = {'id': 'memory_updated', 'status': 'completed', 'text': '祁门红茶', 'world': {'target_seen': True, 'irrelevant_seen': False}}
        self.assertTrue(agent_eval.grade_state(updated))
        updated['text'] = '白毫银针'
        self.assertFalse(agent_eval.grade_state(updated), 'superseded preference is not the current answer')
        answers = {key: {'ground_truth': [{'f': {'x': [1]}}]} for key in agent_eval.HOLDOUT_IDS if not key.startswith('irrelevance_')}
        rows = [{'kind': 'bfcl', 'id': key, 'arm': 'angelbot_prompt', 'status': 'completed', 'requestCount': 1,
                 'calls': [] if key.startswith('irrelevance_') else [{'name': 'f', 'arguments': {'x': 1}}], 'text': 'No applicable function.'}
                for key in agent_eval.HOLDOUT_IDS]
        conflict['world']['revision'] = 19
        updated['text'] = '祁门红茶'
        rows += [dict(task, kind='synthetic_state', arm='agent_runner', requestCount=2) for task in (conflict, updated)]
        status, summary = agent_eval.summarize(rows, answers, 'holdout', ('angelbot_prompt',))
        self.assertEqual(status, 'passed')
        self.assertEqual(summary['angelbot_prompt']['planned'], 8)
        self.assertNotIn('minimal_fc', summary, 'an explicitly skipped baseline must not be run or scored')
        self.assertEqual(agent_eval.summarize(rows, answers, 'holdout')[0], 'blocked', 'default two-arm runs still require both arms')
        status, summary = agent_eval.summarize(rows[:-2], answers, 'holdout', ('angelbot_prompt',), static_only=True)
        self.assertEqual(status, 'passed')
        self.assertEqual(summary['agent_runner'], {'passed': 0, 'evaluated': 0, 'planned': 0})
        self.assertEqual(agent_eval.summarize(rows[:-2], answers, 'holdout', ('angelbot_prompt',))[0], 'blocked')

    def test_companion_probe_exact_scoring_never_counts_offline_or_missing_as_passed(self):
        cases = [{'id': 'sample', 'expected': ['PLAN']}]
        def row(arm, status='completed', answer='PLAN', live=True):
            return {'id': 'sample', 'arm': arm, 'status': status, 'answer': answer,
                    'requestCount': int(live), 'toolCount': 0}
        rows = [row(arm) for arm in companion_eval.ARMS]
        mixed_log = f'test {companion_eval.TEST} ... COMPANION_RESULT ' + json.dumps(rows[0]) + '\nCOMPANION_RESULT ' + json.dumps(rows[1]) + '\nok\n'
        self.assertEqual(companion_eval.parse_rows(mixed_log), rows)
        status, summary = companion_eval.score(cases, rows, True)
        self.assertEqual(status, 'passed')
        self.assertEqual(summary['angelbot_recall']['passed'], 1)
        rows[1]['answer'] = 'Do not PLAN'
        self.assertEqual(companion_eval.score(cases, rows, True)[0], 'needs_work')
        self.assertEqual(companion_eval.score(cases, rows[:1], True)[0], 'blocked')
        offline = [row(arm, 'offline', None, False) for arm in companion_eval.ARMS]
        status, summary = companion_eval.score(cases, offline, False)
        self.assertEqual(status, 'offline_only')
        self.assertEqual(summary['angelbot_recall']['evaluated'], 0)
        for failure in ('quota_exceeded', 'unavailable', 'timed_out', 'invalid_response'):
            blocked = [row('oracle_history', failure, None)]
            status, summary = companion_eval.score(cases, blocked, True)
            self.assertEqual(status, 'blocked')
            self.assertEqual(summary['oracle_history']['evaluated'], 0)
        for invalid in ([rows[0], rows[0]], [row('invented')],
                        [{**rows[0], 'toolCount': 1}], [{**rows[0], 'requestCount': 2}]):
            with self.assertRaises(ValueError):
                companion_eval.score(cases, invalid, True)
        evidence_case = [{'id': 'sample', 'expected': ['PLAN'], 'requiredEvidence': [0], 'forbiddenEvidence': [1]}]
        for indices, expected_status in (([], 'needs_work'), ([0], 'passed'), ([0, 1], 'needs_work')):
            checked = [row('oracle_history'), {**row('angelbot_recall'), 'evidenceSourceIndices': indices}]
            self.assertEqual(companion_eval.score(evidence_case, checked, True)[0], expected_status)

    def test_canonical_checks_isolate_credentials_and_clean_owned_config(self):
        for fail in (False, True):
            roots = []

            def run(_command, **kwargs):
                env = kwargs.get('env')
                self.assertIsNotNone(env, 'canonical checks must supply an isolated environment')
                for key in ('OPENAI_API_KEY', 'AZURE_OPENAI_KEY', 'AWS_ACCESS_KEY_ID',
                            'ANGELBOT_LIVE_MODEL_TEST', 'VITE_ANGELBOT_E2E'):
                    self.assertTrue(key not in env, f'{key} must not be inherited')
                self.assertEqual(env['CARGO_NET_OFFLINE'], 'true')
                self.assertEqual(env['NPM_CONFIG_OFFLINE'], 'true')
                self.assertEqual(env['COMPILER_SYSTEM_MARKER'], 'keep')
                root = Path(env['ANGELBOT_PRIVATE_VERIFY_CONFIG_DIR'])
                self.assertTrue(root.is_absolute() and root.is_dir())
                roots.append(root)
                if fail:
                    raise OSError('owned subprocess failure')
                return SimpleNamespace(returncode=0)

            with patch.dict(os.environ, {'OPENAI_API_KEY': 'fixture-secret',
                                        'AZURE_OPENAI_KEY': 'fixture-azure-secret',
                                        'AWS_ACCESS_KEY_ID': 'fixture-access-id',
                                        'COMPILER_SYSTEM_MARKER': 'keep',
                                        'ANGELBOT_LIVE_MODEL_TEST': '1', 'VITE_ANGELBOT_E2E': '1'}), \
                    patch.object(verify, 'executable_available', return_value=True), \
                    patch.object(verify, 'platform_command', side_effect=lambda command: command), \
                    patch.object(verify.subprocess, 'run', side_effect=run), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(verify.run_check(verify.Check('owned check', ('owned',)), False), not fail)
            self.assertEqual(len(roots), 1)
            self.assertFalse(roots[0].exists(), 'owned config must be cleaned on success and failure')

    def test_native_storage_is_explicit_and_never_passes_by_skipping_on_other_systems(self):
        native_checks = verify.PROFILES['native-storage']
        self.assertEqual(len(native_checks), 1)
        self.assertIn('--exact', native_checks[0].command)
        self.assertIn('--ignored', native_checks[0].command)
        self.assertIn('llm::chatgpt_auth::tests::native_profile_cutover_keeps_windows_vault_binding',
                      native_checks[0].command)
        for profile in ('quick', 'frontend', 'backend', 'full'):
            self.assertNotIn(native_checks[0], verify.PROFILES[profile])
        with patch.object(sys, 'argv', ['verify.py', 'native-storage']), \
                patch.object(verify, 'os', SimpleNamespace(name='posix')), \
                patch.object(verify, 'run_check') as check, \
                contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                verify.main()
            self.assertEqual(error.exception.code, 2)
            check.assert_not_called()
        with patch.object(sys, 'argv', ['verify.py', 'native-storage']), \
                patch.object(verify, 'os', SimpleNamespace(name='nt')), \
                patch.object(verify, 'run_check', return_value=True) as check, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(verify.main(), 0)
            check.assert_called_once_with(native_checks[0], False)

    def test_native_controls_only_never_builds_normal_binary_or_requests_capture(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(sys, "argv", ["native_uia_smoke.py", "--controls-only"]), \
                patch.object(native, "os", SimpleNamespace(name="nt")), \
                patch.object(native, "isolated_environment", return_value={}), \
                patch.object(native, "build_cargo_binary", return_value=Path(directory) / "owned-test.exe") as build, \
                patch.object(native.subprocess, "CREATE_NO_WINDOW", 0, create=True), \
                patch.object(native.subprocess, "run", return_value=SimpleNamespace(stdout=f"{native.TEST_NAME}: test\n")), \
                patch.object(native, "owned_fixture", side_effect=RuntimeError("owned fixture checkpoint")), \
                self.assertRaisesRegex(RuntimeError, "owned fixture checkpoint"):
            native.main()
        build.assert_called_once_with(library_test=True)
        with patch.object(sys, "argv", ["native_uia_smoke.py", "--controls-only", "--capture-only"]), \
                patch.object(native, "build_cargo_binary") as build, \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as exited:
            native.main()
        self.assertEqual(exited.exception.code, 2)
        build.assert_not_called()

        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            artifact=root / "src-tauri/target/native-capture-helper/debug/angelbot.exe"
            output=json.dumps({"reason":"compiler-artifact","target":{"name":"angelbot","kind":["bin"]},"profile":{"test":False},"executable":str(artifact)})
            with patch.object(native,"ROOT",root), patch.object(native,"isolated_environment",return_value={}), \
                    patch.object(native.subprocess,"run",return_value=SimpleNamespace(stdout=output,stderr="",returncode=0)) as build, \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(native.build_cargo_binary(library_test=False),artifact.resolve())
                self.assertEqual(build.call_args.kwargs["env"]["CARGO_TARGET_DIR"],str(root / "src-tauri/target/native-capture-helper"))

    def test_model_recovery_isolates_environment_and_preserves_next_message_gate(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
                "PRIVATE_API_KEY": "PRIVATE_CANARY", "ANGELBOT_NATIVE_UIA_SMOKE": "1",
                "ANGELBOT_DESKTOP_E2E_MANUAL": "1", "VITE_ANGELBOT_LIVE_TEST": "1",
                "ANGELBOT_DESKTOP_E2E_MODEL_RESPONSE": "PRIVATE_CANARY"}, clear=True):
            runtime = Path(directory)
            first = recovery.recovery_environment(runtime, runtime / "evidence", "first")
            self.assertNotIn("PRIVATE_CANARY", json.dumps(first))
            self.assertNotIn("ANGELBOT_DESKTOP_E2E_MANUAL", first)
            self.assertNotIn("ANGELBOT_NATIVE_UIA_SMOKE", first)
            self.assertNotIn("VITE_ANGELBOT_LIVE_TEST", first)
            self.assertEqual(first["APPDATA"], directory)
            self.assertEqual(first["LOCALAPPDATA"], directory)
            self.assertEqual(first["ANGELBOT_DESKTOP_E2E_DATA_DIR"], str(runtime / "angelbot-data"))
            script = json.loads(first["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"])
            self.assertEqual(script[0]["calls"], [{"id": "native-recovery-list", "name": "list_automations", "arguments": {}}])
            self.assertEqual(script[1]["expectUserContentIncludes"], ["native-recovery-initial", "native-recovery-next"])
            reopened = recovery.recovery_environment(runtime, runtime / "evidence", "reopen")
            script = json.loads(reopened["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"])
            self.assertEqual(len(script), 1)
            self.assertEqual(script[0]["type"], "text")
            self.assertEqual(script[0]["expectUserContentIncludes"],
                             ["native-recovery-initial", "native-recovery-next", "native-recovery-reopen"])
            with self.assertRaises(ValueError):
                recovery.recovery_environment(runtime, runtime / "evidence", "typo")

    def test_project_restore_isolates_profiles_and_never_reuses_stale_passes(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {
                "PRIVATE_API_KEY": "PRIVATE_CANARY", "ANGELBOT_DESKTOP_E2E_MANUAL": "1",
                "VITE_ANGELBOT_LIVE_TEST": "1", "ANGELBOT_DESKTOP_E2E_MODEL_RESPONSE": "PRIVATE_CANARY"}, clear=True):
            runtime = Path(directory)
            first = restore.restore_environment(runtime, runtime / "evidence", "source")
            target = restore.restore_environment(runtime, runtime / "evidence", "target")
            reopened = restore.restore_environment(runtime, runtime / "evidence", "reopen")
            self.assertNotIn("PRIVATE_CANARY", json.dumps(first))
            self.assertNotIn("ANGELBOT_DESKTOP_E2E_MANUAL", first)
            self.assertNotIn("VITE_ANGELBOT_LIVE_TEST", first)
            self.assertEqual(first["APPDATA"], str(runtime / "source"))
            self.assertEqual(target["APPDATA"], str(runtime / "target"))
            self.assertEqual(target["ANGELBOT_DESKTOP_E2E_DATA_DIR"], reopened["ANGELBOT_DESKTOP_E2E_DATA_DIR"])
            self.assertNotEqual(first["ANGELBOT_DESKTOP_E2E_DATA_DIR"], target["ANGELBOT_DESKTOP_E2E_DATA_DIR"])
            with self.assertRaises(ValueError):
                restore.restore_environment(runtime, runtime / "evidence", "typo")

        for failure in ("build", "driver", "incomplete"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                evidence = root / "src-tauri/target/e2e/project-restore"
                evidence.mkdir(parents=True)
                for name in ("acceptance", *restore.REQUIRED):
                    (evidence / f"{name}.json").write_text('{"status":"passed"}')
                error = subprocess.CalledProcessError(2, "PRIVATE_STARTUP_ERROR")
                def driver(*_args, **_kwargs):
                    if failure == "driver":
                        raise error
                def prepare(*_args):
                    if failure == "build":
                        raise error

                with patch.object(restore.subprocess, "run", side_effect=driver), \
                        contextlib.redirect_stdout(io.StringIO()), \
                        self.assertRaises(RuntimeError if failure == "incomplete" else subprocess.CalledProcessError):
                    restore.run_project_restore(root, prepare, lambda _env: None, False, False)
                report = json.loads((evidence / "acceptance.json").read_text())
                self.assertEqual(report["status"], "failed")
                self.assertFalse(report["phases"])
                self.assertNotIn("PRIVATE_STARTUP_ERROR", json.dumps(report))
                self.assertEqual(json.loads((evidence / "stage.json").read_text())["phase"], "failed")
                for name in ("target", "reopen"):
                    self.assertEqual(json.loads((evidence / f"{name}.json").read_text())["status"], "not_started")

    def test_restore_host_requires_exact_history_scope_and_no_replay(self):
        expected = {table: [] for table in restore_assertions.TABLES}
        for identity in ("personal", "a", "b"):
            expected["projects"].append({"id": identity, "kind": "personal" if identity == "personal" else "project",
                "path": None if identity == "personal" else f"/old-{identity}",
                "directory_confirmed": int(identity == "personal"), "active_session_id": f"{identity}-main"})
            expected["sessions"].append({"id": f"{identity}-main", "project_id": identity,
                "parent_id": None, "leaf_message_id": "reply" if identity == "a" else None,
                "work_dir": None, "additional_read_dirs": "[]",
                "title": "" if identity == "personal" else f"{identity} title"})
        expected["messages"] = [{"id": "reply", "session_id": "a-main", "parent_id": None,
            "role": "assistant", "content": "fixed synthetic history", "is_provisional": 0},
            {"id": "daily-user", "session_id": "personal-main", "parent_id": None,
             "role": "user", "content": "daily title canary", "is_provisional": 0}]
        expected["task_runs"] = [{"id": "reply", "session_id": "a-main", "message_id": "reply",
            "status": "completed", "confirmation_state": "none", "resumable": 0}]
        expected["task_run_facts"] = [{"task_run_id": "reply", "session_id": "a-main", "message_id": "reply",
            "facts_json": json.dumps({"goal": "fixed synthetic goal"})}]
        completed_facts = {"goal": "fixed synthetic goal", "plan": [], "completed_steps": [],
            "failed_steps": [], "modified_files": [], "verification_evidence": [],
            "pending_confirmation": None, "repair_attempts": 0, "no_progress_count": 0,
            "context_summary": None, "provider_attempts": 0, "provider_binding": None,
            "terminal_reason": None}
        manifest = {"expected": expected,
            "projectA": {"id": "a", "sessionId": "a-main", "mappedPath": "/mapped-a"},
            "projectB": {"id": "b", "sessionId": "b-main", "originalPath": "/old-b"}}
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "target.db"
            source = Path(directory) / "source.db"
            with contextlib.closing(sqlite3.connect(target)) as db:
                for table, rows in expected.items():
                    columns = list(rows[0]) if rows else ["task_run_id" if table == "task_run_facts" else "id"]
                    db.execute(f'CREATE TABLE "{table}" ({",".join(columns)})')
                    for row in rows:
                        observed = dict(row)
                        if table == "projects" and row["id"] == "a":
                            observed.update(path="/mapped-a", directory_confirmed=1)
                        if table == "sessions" and row["project_id"] == "a":
                            observed["work_dir"] = "/mapped-a"
                        if table == "sessions" and row["id"] == "personal-main":
                            observed["title"] = "daily title canary"
                        if table == "task_run_facts":
                            observed["facts_json"] = json.dumps(completed_facts)
                        db.execute(f'INSERT INTO "{table}" VALUES ({",".join("?" for _ in columns)})', list(observed.values()))
                for table in restore_assertions.EMPTY_QUEUES:
                    db.execute(f'CREATE TABLE "{table}" (id TEXT)')
                for table in ("automations", "scheduled_tasks"):
                    db.execute(f'CREATE TABLE "{table}" (id TEXT, enabled INTEGER)')
                db.commit()
                self.assertTrue(restore_assertions.check_restored_database(target, source, manifest)["noReplay"])
                for sql in ("UPDATE projects SET directory_confirmed=1 WHERE id='b'",
                            "UPDATE messages SET content='altered'",
                            "UPDATE sessions SET title='wrong title' WHERE id='personal-main'",
                            "UPDATE sessions SET title='wrong title' WHERE id='a-main'",
                            f"UPDATE task_run_facts SET facts_json='{json.dumps({**completed_facts, 'terminal_reason': 'needs_attention'})}'",
                            "INSERT INTO agent_steps(id) VALUES('unexpected')",
                            "INSERT INTO automation_runs(id) VALUES('replay')",
                            "INSERT INTO automations(id,enabled) VALUES('timer',1)",
                            "DROP TABLE workspace_supervisor_work"):
                    with self.subTest(sql=sql):
                        db.execute(sql)
                        # A second read-only connection must observe the change.
                        db.commit()
                        with self.assertRaises(AssertionError):
                            restore_assertions.check_restored_database(target, source, manifest)
                        if sql.startswith("DROP"):
                            db.execute("CREATE TABLE workspace_supervisor_work (id TEXT)")
                        else:
                            db.execute("UPDATE projects SET directory_confirmed=0 WHERE id='b'")
                            db.executemany("UPDATE messages SET content=? WHERE id=?",
                                           [(row["content"], row["id"]) for row in expected["messages"]])
                            db.execute("UPDATE sessions SET title=CASE id WHEN 'personal-main' THEN 'daily title canary' WHEN 'a-main' THEN 'a title' ELSE title END")
                            db.execute("UPDATE task_run_facts SET facts_json=?", (json.dumps(completed_facts),))
                            db.execute("DELETE FROM agent_steps")
                            db.execute("DELETE FROM automation_runs")
                            db.execute("DELETE FROM automations")
                        db.commit()
                with self.assertRaisesRegex(AssertionError, "paths must differ"):
                    restore_assertions.check_restored_database(target, target, manifest)
                expected["sessions"][0]["title"] = "existing daily title"
                with self.assertRaisesRegex(AssertionError, "exact sessions.title for personal-main"):
                    restore_assertions.check_restored_database(target, source, manifest)
                db.execute("UPDATE sessions SET title='existing daily title' WHERE id='personal-main'")
                db.commit()
                self.assertTrue(restore_assertions.check_restored_database(target, source, manifest)["noReplay"])
                for source_status, source_reason in (("running", None), ("completed", "awaiting_confirmation")):
                    with self.subTest(source_status=source_status, source_reason=source_reason):
                        expected["task_runs"][0]["status"] = source_status
                        expected["task_runs"][0]["resumable"] = 1
                        expected["task_run_facts"][0]["facts_json"] = json.dumps({
                            "goal": "fixed synthetic goal", "terminal_reason": source_reason})
                        db.execute("UPDATE task_runs SET status=?, resumable=0",
                                   ("needs_attention" if source_status == "running" else "completed",))
                        db.execute("UPDATE task_run_facts SET facts_json=?", (json.dumps({
                            **completed_facts, "terminal_reason": "needs_attention"}),))
                        db.commit()
                        self.assertTrue(restore_assertions.check_restored_database(target, source, manifest)["noReplay"])
                        db.execute("UPDATE task_run_facts SET facts_json=?", (json.dumps(completed_facts),))
                        db.commit()
                        with self.assertRaisesRegex(AssertionError, "exact task_run_facts.facts_json"):
                            restore_assertions.check_restored_database(target, source, manifest)

    def test_saved_model_probe_is_single_request_and_never_starts_browser_tasks(self):
        for status, count, extra, exit_code in (("available", 1, {}, 0), ("quota_exceeded", 1, {}, 2),
                ("unavailable", 0, {}, 2), ("unavailable", 1, {}, 2), ("timed_out", 1, {}, 2),
                ("available", 0, {}, None), ("available", 1, {"diagnostic": "PRIVATE_CANARY"}, None)):
            with self.subTest(status=status, count=count, extra=extra), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                def driver(command, **kwargs):
                    self.assertEqual(command[1], 'commands::message::live_tests::configured_model_completes_bounded_file_task')
                    self.assertEqual(kwargs['env']['ANGELBOT_LIVE_MODEL_JOURNEY'], 'model-probe')
                    self.assertNotIn('PRIVATE_CANARY', json.dumps(kwargs['env']))
                    self.assertNotIn('ANGELBOT_PRIVATE_LIVE_PHASE', kwargs['env'])
                    self.assertNotIn('ANGELBOT_LIVE_BROWSER_CLI', kwargs['env'])
                    self.assertEqual(kwargs['timeout'], 75)
                    kwargs['stdout'].write('MODEL_AVAILABILITY_REPORT ' + json.dumps({
                        'status': status, 'requestCount': count, 'toolCount': 0, **extra}) + '\n')
                    return SimpleNamespace(returncode=0)
                with patch.object(browser_acceptance, 'ROOT', root), \
                        patch.object(browser_acceptance, 'build_cargo_binary', return_value=root / 'owned-test.exe') as build, \
                        patch.object(browser_acceptance.subprocess, 'run', side_effect=driver) as run, \
                        patch.object(browser_acceptance, 'serve_fixture') as server, \
                        patch.dict(os.environ, {'PRIVATE_API_KEY': 'PRIVATE_CANARY',
                            'ANGELBOT_PRIVATE_LIVE_PHASE': 'browser-write',
                            'ANGELBOT_LIVE_BROWSER_CLI': 'PRIVATE_CANARY'}), \
                        contextlib.redirect_stdout(io.StringIO()) as output:
                    if exit_code is None:
                        with self.assertRaisesRegex(RuntimeError, 'invalid bounded report'):
                            browser_acceptance.run_model_probe()
                    else:
                        self.assertEqual(browser_acceptance.run_model_probe(), exit_code)
                build.assert_called_once_with(library_test=True)
                run.assert_called_once()
                server.assert_not_called()
                report, = (root / 'logs').glob('model-probe-*/acceptance.json')
                body = json.loads(report.read_text())
                self.assertEqual(body['status'], status if exit_code is not None else 'failed')
                self.assertNotIn('PRIVATE_CANARY', report.read_text() + output.getvalue())
        with patch.object(sys, 'argv', ['browser_form_acceptance.py', '--model-probe']), \
                patch.object(browser_acceptance, 'run_model_probe', return_value=7) as start:
            self.assertEqual(browser_acceptance.main(), 7)
            start.assert_called_once()
        for arguments in ([], ['--model-probe', '--daily-report'], ['--model-probe', '--hold-for-inspection']):
            with self.subTest(arguments=arguments), patch.object(sys, 'argv', ['browser_form_acceptance.py', *arguments]), \
                    patch.object(browser_acceptance, 'run_model_probe') as start, \
                    contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                browser_acceptance.main()
            start.assert_not_called()

    def test_recorded_inference_probe_never_starts_browser_or_accepts_execution(self):
        for tool_count, diagnostic in ((0, None), (1, None), (0, 'diag:PRIVATE_CANARY')):
            with self.subTest(tool_count=tool_count, diagnostic=diagnostic), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / 'data.db'
                source.write_bytes(b'owned synthetic database')
                def driver(command, **kwargs):
                    self.assertEqual(kwargs['env']['ANGELBOT_LIVE_MODEL_JOURNEY'], 'recorded-inference')
                    self.assertEqual(kwargs['env']['ANGELBOT_PRIVATE_REPLAY_DB'], str(source.resolve()))
                    self.assertNotIn('ANGELBOT_LIVE_BROWSER_CLI', kwargs['env'])
                    self.assertNotIn('PRIVATE_API_KEY', kwargs['env'])
                    kwargs['stdout'].write('RECORDED_INFERENCE_REPORT ' + json.dumps({
                        'status':'available', 'requestCount':1, 'toolCount':tool_count,
                        'historyCount':3, 'schemaCount':2, 'diagnosticRef':diagnostic,
                        'reconstructedNotByteExact':True}) + '\n')
                    return SimpleNamespace(returncode=0)
                with patch.object(browser_acceptance, 'ROOT', root), \
                        patch.object(browser_acceptance, 'build_cargo_binary', return_value=root / 'owned-test.exe'), \
                        patch.object(browser_acceptance.subprocess, 'run', side_effect=driver) as run, \
                        patch.object(browser_acceptance, 'serve_fixture') as server, \
                        patch.dict(os.environ, {'PRIVATE_API_KEY':'PRIVATE_CANARY'}), \
                        contextlib.redirect_stdout(io.StringIO()) as output:
                    if tool_count or diagnostic:
                        with self.assertRaisesRegex(RuntimeError, 'invalid bounded report'):
                            browser_acceptance.run_model_probe(source)
                    else:
                        self.assertEqual(browser_acceptance.run_model_probe(source), 0)
                run.assert_called_once()
                server.assert_not_called()
                report, = (root / 'logs').glob('inference-replay-*/acceptance.json')
                self.assertNotIn('PRIVATE_CANARY', report.read_text() + output.getvalue())
                self.assertEqual(source.read_bytes(), b'owned synthetic database')

    def test_restore_only_reuses_source_evidence_without_running_old_phases(self):
        for alter_original in (False, True):
            with self.subTest(alter_original=alter_original), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                previous = root / 'old-report'
                previous.mkdir()
                retained = root / 'retained'
                marker = str(browser_acceptance.uuid.uuid4())
                raw = b'item,count\ncable,1\n'
                attachment = dict(name='borrow-items.csv', bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())
                draft = dict(id=1, title='Synthetic', category='equipment', location='Room',
                             date='2026-10-23', notes='Draft only', adapter=True, attachment=attachment)
                business = dict(drafts=[draft], mutations=1, saveAttempts=1, receiptLost=True,
                                downloadAttempts=0, uploadAttempts=1)
                receipt = {k: v for k, v in draft.items() if k != 'id'}
                receipt.update(draft_id=1, status='draft')
                (previous / 'state.json').write_text(json.dumps(business))
                (previous / 'upload-1.csv').write_bytes(raw)
                (previous / 'live.log').write_text(f'owned fixture retained at {retained}\n')
                for parent in (retained, retained / 'restored'):
                    (parent / 'results').mkdir(parents=True)
                    (parent / 'data.db').write_bytes(b'original database evidence')
                    for name in ('request.json', 'receipt.json', 'borrow-items.csv'):
                        content = json.dumps(receipt).encode() if name == 'receipt.json' else raw
                        (parent / 'results' / name).write_bytes(content)
                for name in ('browser-write.passed', 'browser-reopen.passed'):
                    (retained / name).write_text(marker)
                written = dict(phase='source-to-web-to-report', sessionId='personal-main',
                               successfulWrites=1, receipt=receipt, sourceUnchanged=True)
                reopened = {**written, 'phase':'reopen-read-only', 'backupRestoreVerified':True}
                source_report = dict(status='failed', ownedRoot=str(previous), ownedServerClosed=True,
                                     projectReceipts=[written, reopened], businessState=business)
                report_path = previous / 'acceptance.json'
                report_path.write_text(json.dumps(source_report))
                cli = root / 'cli.js'
                cli.touch()
                (root / 'package.json').write_text(json.dumps({'name':'@playwright/mcp', 'version':'0.0.83'}))
                def driver(_command, **kwargs):
                    env = kwargs['env']
                    phase = env['ANGELBOT_PRIVATE_LIVE_PHASE']
                    self.assertIn(phase, ('browser-prepare', 'browser-restored'))
                    self.assertEqual(env['ANGELBOT_PRIVATE_RETAINED_SOURCE'], str(retained.resolve()))
                    self.assertNotIn('PRIVATE_API_KEY', env)
                    owned = Path(env['ANGELBOT_PRIVATE_LIVE_ROOT'])
                    (owned / f'{phase}.passed').write_text(env['ANGELBOT_PRIVATE_LIVE_RECEIPT'])
                    if phase == 'browser-prepare':
                        return SimpleNamespace(returncode=0)
                    self.assertEqual(json.loads((owned / 'state.json').read_text()), business)
                    new = {**draft, 'id':2, 'title':'恢复后复核：' + draft['title']}
                    state = {**business, 'drafts':[draft, new], 'saveAttempts':2, 'mutations':2, 'uploadAttempts':2}
                    (owned / 'state.json').write_text(json.dumps(state))
                    (owned / 'upload-2.csv').write_bytes(raw)
                    restored = dict(sessionId='personal-main', successfulWrites=1,
                                    report=json.dumps({**receipt, 'draft_id':2, 'title':new['title']}),
                                    **dict.fromkeys(('sameMain', 'oldHistoryUnchanged', 'sourceFilesUnchanged',
                                                     'newTaskCompleted', 'mcpStarted', 'mcpAuthorizationRenewed'), True))
                    kwargs['stdout'].write('LIVE_BROWSER_RESTORED_REPORT ' + json.dumps(restored) + '\n')
                    if alter_original:
                        (retained / 'restored' / 'data.db').write_bytes(b'changed old failure evidence')
                    return SimpleNamespace(returncode=0)
                with patch.object(browser_acceptance, 'ROOT', root), \
                        patch.object(browser_acceptance, 'build_cargo_binary', return_value=root / 'test.exe'), \
                        patch.object(browser_acceptance.subprocess, 'run', side_effect=driver) as run, \
                        patch.dict(os.environ, {'PRIVATE_API_KEY':'PRIVATE_CANARY'}), \
                        patch.object(sys, 'argv', ['browser_form_acceptance.py', '--restore-only', str(report_path), '--mcp-cli', str(cli)]), \
                        contextlib.redirect_stdout(io.StringIO()):
                    if alter_original:
                        with self.assertRaisesRegex(RuntimeError, 'original failure evidence changed'):
                            browser_acceptance.main()
                    else:
                        self.assertEqual(browser_acceptance.main(), 0)
                self.assertEqual(run.call_count, 2)
                self.assertEqual([call.kwargs['env']['ANGELBOT_PRIVATE_LIVE_PHASE'] for call in run.call_args_list],
                                 ['browser-prepare', 'browser-restored'])
                result, = (root / 'logs').glob('browser-form-*/acceptance.json')
                result = json.loads(result.read_text(encoding='utf-8'))
                self.assertEqual(result['retainedSourceAndFailedTargetUnchanged'], not alter_original)
                self.assertEqual(result['status'], 'failed' if alter_original else 'passed_with_independent_server_check')
                self.assertEqual(json.loads(report_path.read_text()), source_report)
                self.assertEqual(json.loads((previous / 'state.json').read_text()), business)

    def test_continue_delivery_retains_main_history_and_refuses_replay(self):
        for failure in (None, 'old-history', 'replay'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                # Exercise a noncanonical spelling even where NTFS 8.3 aliases
                # are disabled; the existing owned directory still resolves identically.
                root = Path(directory) / '..' / Path(directory).name
                previous = root / 'prior'
                results = previous / 'restored/results'
                results.mkdir(parents=True)
                (previous / 'browser-prepare.passed').write_text(str(browser_acceptance.uuid.uuid4()))
                raw = b'item,count\ncable,1\n'
                receipt = dict(draft_id=1, status='draft', title='Synthetic', category='equipment', location='Room',
                               date='2026-10-23', notes='Draft only', adapter=True,
                               attachment=dict(name='borrow-items.csv', bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest()))
                delivered = {**receipt, 'draft_id':2, 'title':'恢复后复核：Synthetic'}
                for name, data in (('request.json', b'{}'), ('receipt.json', json.dumps(receipt).encode()),
                                   ('borrow-items.csv', raw), ('restored-upload.json', json.dumps(delivered).encode())):
                    (results / name).write_bytes(data)
                drafts = [{**{k:v for k,v in value.items() if k not in ('draft_id','status')}, 'id':value['draft_id']}
                          for value in (receipt, delivered)]
                business = dict(drafts=drafts, mutations=2, saveAttempts=2, uploadAttempts=2,
                                receiptLost=True, downloadAttempts=0)
                with browser_acceptance.socket.socket() as reserve:
                    reserve.bind(('127.0.0.1', 0))
                    port = reserve.getsockname()[1]
                url = f'http://127.0.0.1:{port}/daily/{browser_acceptance.uuid.uuid4().hex}/'
                source_report = dict(status='failed', ownedRoot=str(previous), ownedServerClosed=True,
                                     retainedSourceAndFailedTargetUnchanged=True, businessState=business,
                                     projectReceipts=[], url=url)
                # Wrapper-only fixture: actual backend tests use the full migrated schema.
                database = results.parent / 'data.db'
                with contextlib.closing(sqlite3.connect(database)) as conn:
                    conn.executescript("CREATE TABLE messages(id TEXT,role TEXT,content TEXT);"
                        "CREATE TABLE sessions(id TEXT,leaf_message_id TEXT);"
                        "CREATE TABLE task_runs(message_id TEXT,session_id TEXT,goal TEXT,status TEXT);"
                        "INSERT INTO messages VALUES('old','assistant','original reply');"
                        "INSERT INTO sessions VALUES('personal-main','old');"
                        "INSERT INTO task_runs VALUES('old','personal-main','original goal','needs_attention');")
                    conn.commit()
                report_path = previous / 'acceptance.json'
                report_path.write_text(json.dumps(source_report))
                (previous / 'state.json').write_text(json.dumps(business))
                (previous / 'live.log').write_text('original failed attempt')
                for index in (1, 2):
                    (previous / f'upload-{index}.csv').write_bytes(raw)
                cli = root / 'cli.js'
                cli.touch()
                (root / 'package.json').write_text(json.dumps({'name':'@playwright/mcp', 'version':'0.0.83'}))
                def driver(_command, **kwargs):
                    env = kwargs['env']
                    self.assertEqual(env['ANGELBOT_PRIVATE_LIVE_PHASE'], 'browser-continue')
                    self.assertEqual(Path(env['ANGELBOT_PRIVATE_CONTINUATION_PROFILE']), results.parent.resolve())
                    owned = Path(env['ANGELBOT_PRIVATE_LIVE_ROOT'])
                    (owned / 'browser-continue.passed').write_text(env['ANGELBOT_PRIVATE_LIVE_RECEIPT'])
                    with contextlib.closing(sqlite3.connect(database)) as conn:
                        conn.execute("INSERT INTO messages VALUES('new','assistant','Report: angelbot-file:restored-upload.json')")
                        conn.execute("INSERT INTO task_runs VALUES('new','personal-main','original goal','completed')")
                        conn.execute("UPDATE sessions SET leaf_message_id='new'")
                        if failure == 'old-history':
                            conn.execute("UPDATE messages SET content='changed' WHERE id='old'")
                        conn.commit()
                    if failure == 'replay':
                        parsed = browser_acceptance.urlsplit(url)
                        connection = http.client.HTTPConnection('127.0.0.1', parsed.port, timeout=2)
                        try:
                            connection.request('POST', parsed.path + 'api/drafts', b'{}')
                            response = connection.getresponse()
                            self.assertEqual(response.status, 409)
                            response.read()
                        finally:
                            connection.close()
                    kwargs['stdout'].write('LIVE_BROWSER_CONTINUED_REPORT ' + json.dumps(dict(
                        sessionId='personal-main', sourceMessageId='old', replyMessageId='new',
                        taskCompleted=True, goalUnchanged=True, calls=0, mcpStarted=False)) + '\n')
                    return SimpleNamespace(returncode=0)
                with patch.object(browser_acceptance, 'ROOT', root), \
                        patch.object(browser_acceptance, 'build_cargo_binary', return_value=root / 'test.exe'), \
                        patch.object(browser_acceptance.subprocess, 'run', side_effect=driver) as run, \
                        patch.object(sys, 'argv', ['browser_form_acceptance.py', '--continue-delivery', str(report_path), '--mcp-cli', str(cli)]), \
                        contextlib.redirect_stdout(io.StringIO()):
                    if failure == 'old-history':
                        with self.assertRaisesRegex(RuntimeError, 'Old Main history changed'):
                            browser_acceptance.main()
                    elif failure == 'replay':
                        with self.assertRaises(AssertionError):
                            browser_acceptance.main()
                    else:
                        self.assertEqual(browser_acceptance.main(), 0)
                run.assert_called_once()
                outcome, = (root / 'logs').glob('browser-form-*/acceptance.json')
                outcome = json.loads(outcome.read_text(encoding='utf-8'))
                self.assertEqual(outcome['status'], 'failed' if failure else 'passed_read_only_main_continuation')
                self.assertTrue(outcome['ownedServerClosed'])
                self.assertTrue(outcome['existingDeliveryAndEvidenceFilesUnchanged'])
                self.assertEqual(json.loads(report_path.read_text()), source_report)

    def test_upload_fixture_checks_real_bytes_and_exposes_duplicate_saves(self):
        with tempfile.TemporaryDirectory() as directory:
            owned = Path(directory)
            server, prefix = browser_acceptance.serve_fixture(owned, attachment_upload=True)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            def request(method, route, payload=None):
                connection = http.client.HTTPConnection('127.0.0.1', server.server_port, timeout=2)
                try:
                    data = json.dumps(payload).encode() if payload is not None else None
                    connection.request(method, prefix + route, data, {'Content-Type': 'application/json'})
                    response = connection.getresponse()
                    return response.status, response.read().decode()
                finally:
                    connection.close()
            try:
                code, review = request('GET', 'review')
                self.assertEqual(code, 200)
                self.assertIn('type="file"', review)
                # Compile fixture JS without a DOM, browser, model or package download.
                import re
                scripts = re.findall(r'<script>(.*?)</script>', review, re.S)
                self.assertTrue(scripts)
                subprocess.run([shutil.which('node'), '-e',
                    'for(const source of JSON.parse(require("fs").readFileSync(0,"utf8"))) new (require("vm").Script)(source)'],
                    input=json.dumps(scripts), text=True, encoding='utf-8', capture_output=True, check=True,
                    timeout=10, env=verify.isolated_environment(),
                    creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
                draft = dict(title='合成设备申请', category='equipment', location='二楼会议室',
                             date='2026-10-23', notes='只保存草稿', adapter=True)
                raw = '物品,数量\nHDMI 转接线,1\n'.encode()
                attachment = {'name': 'borrow-items.csv', 'data': base64.b64encode(raw).decode()}
                for extra in (None, {**attachment, 'name': '../escape.csv'},
                              {**attachment, 'data': '!invalid!'},
                              {**attachment, 'data': base64.b64encode(b'x' * 4097).decode()}):
                    payload = draft if extra is None else {**draft, 'attachment': extra}
                    self.assertEqual(request('POST', 'api/drafts', payload)[0], 400)
                self.assertEqual(json.loads((owned / 'state.json').read_text(encoding='utf-8'))['mutations'], 0)
                payload = {**draft, 'attachment': attachment}
                with self.assertRaises((http.client.RemoteDisconnected, ConnectionResetError)):
                    request('POST', 'api/drafts', payload)
                state = json.loads((owned / 'state.json').read_text(encoding='utf-8'))
                self.assertEqual(state['uploadAttempts'], 1)
                self.assertEqual(state['saveAttempts'], 1)
                self.assertTrue(state['receiptLost'])
                expected = {'name': 'borrow-items.csv', 'bytes': len(raw),
                            'sha256': hashlib.sha256(raw).hexdigest()}
                self.assertEqual(state['drafts'][0]['attachment'], expected)
                self.assertEqual((owned / 'upload-1.csv').read_bytes(), raw)
                self.assertIn(expected['sha256'], request('GET', 'drafts/1')[1])
                # Do not add fixture-side idempotency that would hide an Agent replay.
                self.assertEqual(request('POST', 'api/drafts', payload)[0], 200)
                state = json.loads((owned / 'state.json').read_text(encoding='utf-8'))
                self.assertEqual(state['saveAttempts'], 2)
                self.assertEqual(state['uploadAttempts'], 2)
                self.assertEqual(state['mutations'], 2)
                self.assertEqual(len(state['drafts']), 2)
                self.assertEqual((owned / 'upload-2.csv').read_bytes(), raw)
                # The restored journey permits one explicitly new draft, not
                # a replay of the old one. Reject misleading completion proof.
                receipt = {**state['drafts'][0], 'draft_id': 1, 'status': 'draft'}
                receipt.pop('id')
                written = {'sessionId': 'personal-main', 'receipt': receipt, 'successfulWrites': 1}
                reopened = {**written, 'backupRestoreVerified': True}
                new_receipt = {**receipt, 'draft_id': 2, 'title': '恢复后复核：' + receipt['title']}
                restored = {'sessionId': 'personal-main', 'successfulWrites': 1,
                            'report': json.dumps(new_receipt),
                            **dict.fromkeys(('sameMain', 'oldHistoryUnchanged', 'sourceFilesUnchanged',
                                             'newTaskCompleted', 'mcpStarted', 'mcpAuthorizationRenewed'), True)}
                business = json.loads(json.dumps(state))
                business['drafts'][1]['title'] = new_receipt['title']
                browser_acceptance.verify_restored_upload(owned, written, reopened, restored, business)
                for field, value in (('sameMain', False), ('mcpAuthorizationRenewed', False),
                                     ('sourceFilesUnchanged', False), ('successfulWrites', 0),
                                     ('report', json.dumps(receipt))):
                    with self.subTest(restored_field=field), self.assertRaises(AssertionError):
                        browser_acceptance.verify_restored_upload(
                            owned, written, reopened, {**restored, field: value}, business)
                for field, value in (('saveAttempts', 3), ('downloadAttempts', 1), ('uploadAttempts', 1)):
                    with self.subTest(business_field=field), self.assertRaises(AssertionError):
                        browser_acceptance.verify_restored_upload(
                            owned, written, reopened, restored, {**business, field: value})
                (owned / 'upload-2.csv').write_bytes(raw + b'changed')
                with self.assertRaises(AssertionError):
                    browser_acceptance.verify_restored_upload(owned, written, reopened, restored, business)
            finally:
                server.shutdown()
                server.server_close()
                thread.join(timeout=5)
            self.assertFalse(thread.is_alive())
            with self.assertRaises(ValueError):
                browser_acceptance.serve_fixture(owned, saved_draft=True, attachment_upload=True)

    def test_manual_native_profile_and_environment_never_copy_secrets_or_scripts(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "profile.json"
            profile = {"provider": "openai", "model": "saved-test-model", "base_url": "https://example.invalid",
                       "max_tokens": 4096, "temperature": 0.7, "auth_mode": "chatgpt_plan",
                       "credential_ref": "opaque-handle", "protocol": "openai_responses",
                       "api_key": "PRIVATE_CANARY", "plan_account_label": "PRIVATE_CANARY"}
            source.write_text(json.dumps(profile), encoding="utf-8")
            saved = manual.saved_plan_profile(source)
            self.assertEqual(saved["credential_ref"], "opaque-handle")
            self.assertNotIn("PRIVATE_CANARY", json.dumps(saved))
            for field, value in (("auth_mode", "api_key"), ("credential_ref", ""), ("provider", "custom")):
                source.write_text(json.dumps({**profile, field: value}), encoding="utf-8")
                with self.assertRaises(RuntimeError):
                    manual.saved_plan_profile(source)
            with patch.dict(os.environ, {"PRIVATE_API_KEY": "PRIVATE_CANARY", "APPDATA": "original",
                    "ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT": "script", "ANGELBOT_DESKTOP_E2E_MODEL_RESPONSE": "fake",
                    "ANGELBOT_NATIVE_UIA_SMOKE": "1", "VITE_ANGELBOT_DESKTOP_E2E": "1"}, clear=True):
                env = manual.manual_environment(Path(directory), manual.isolated_environment())
            self.assertNotIn("PRIVATE_CANARY", json.dumps(env))
            self.assertEqual(env["APPDATA"], "original")
            self.assertEqual({key for key in env if key.startswith(("ANGELBOT_", "VITE_ANGELBOT_"))},
                             {"ANGELBOT_DESKTOP_E2E_MANUAL", "ANGELBOT_DESKTOP_E2E_DATA_DIR"})

    def test_manual_native_requires_durable_approval_actual_readback_and_main_completion(self):
        baseline = {key: 0 for key in manual.UNCHANGED}
        baseline.update({"passwordUnchanged": True, "processId": 12, "windowTitle": "owned", "ordinaryValue": "ordinary-seed"})
        manual.check_snapshot(baseline, baseline, 12, "owned", "ordinary-seed")
        for key, value in (("processId", 99), ("ordinaryValue", "wrong"), ("passwordUnchanged", False), ("invocationCount", 1)):
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                manual.check_snapshot({**baseline, key: value}, baseline, 12, "owned", "ordinary-seed")
        with tempfile.TemporaryDirectory() as directory:
            db = Path(directory) / "data.db"
            executable = Path(directory) / "fixture.exe"
            executable.write_bytes(b"not an executable; identity only")
            with contextlib.closing(sqlite3.connect(db)) as conn:
                conn.executescript("""CREATE TABLE agent_steps(call_id TEXT,session_id TEXT,tool_name TEXT,tool_input TEXT,tool_output TEXT,success INTEGER);
                    CREATE TABLE task_runs(message_id TEXT,session_id TEXT,status TEXT,created_at INTEGER);
                    CREATE TABLE messages(id TEXT,content TEXT);
                    CREATE TABLE automation_runs(id TEXT);
                    CREATE TABLE desktop_trusted_apps(id TEXT,executable_path TEXT,enabled INTEGER,capabilities TEXT);
                    CREATE TABLE agent_permission_grants(scope TEXT,permission TEXT);
                    INSERT INTO messages VALUES('reply','已填写并回读核验');
                    INSERT INTO task_runs VALUES('reply','main','completed',1);""")
                conn.execute("INSERT INTO desktop_trusted_apps VALUES(?,?,1,?)", (manual.APP_ID, str(executable), '["observe","fill"]'))
                conn.execute("INSERT INTO agent_steps VALUES('reject','main','set_trusted_app_text',?,'Confirmation rejected',0)",
                             (json.dumps({"app_id": manual.APP_ID, "text": "rejected"}),))
                conn.commit()
                self.assertTrue(manual.check_receipts(db, "rejected", "approved", executable, final=False)["rejectionVerified"])
                receipt = {"adapter": "windows-native-uia", "target": manual.APP_ID, "status": "verified"}
                approved = "Confirmation approved: " + json.dumps(receipt)
                conn.execute("INSERT INTO agent_steps VALUES('approve','main','set_trusted_app_text',?,?,1)",
                             (json.dumps({"app_id": manual.APP_ID, "text": "approved"}), approved))
                conn.execute("INSERT INTO agent_steps VALUES('agent-loop-00000000-0000-4000-8000-000000000001','main','agent_loop','null','host terminal diagnostic',0)")
                conn.commit()
                self.assertTrue(manual.check_receipts(db, "rejected", "approved", executable, final=True)["mainDelivered"])
                for broken in ({**receipt, "adapter": "mock"}, {**receipt, "status": "dispatched"},
                               {**receipt, "target": "another-app"}):
                    conn.execute("UPDATE agent_steps SET tool_output=? WHERE call_id='approve'",
                                 ("Confirmation approved: " + json.dumps(broken),))
                    conn.commit()
                    with self.assertRaises(RuntimeError):
                        manual.check_receipts(db, "rejected", "approved", executable, final=True)
                conn.execute("UPDATE agent_steps SET tool_output=? WHERE call_id='approve'", (approved,))
                for query in ("UPDATE task_runs SET status='awaiting_confirmation'",
                              "INSERT INTO automation_runs VALUES('unexpected')",
                              "INSERT INTO agent_steps SELECT 'replay',session_id,tool_name,tool_input,tool_output,success FROM agent_steps WHERE call_id='approve'",
                              "INSERT INTO agent_permission_grants VALUES('global','full_access')",
                              "UPDATE agent_steps SET tool_input='{}' WHERE tool_name='agent_loop'",
                              "UPDATE agent_steps SET success=1 WHERE tool_name='agent_loop'",
                              "UPDATE agent_steps SET call_id='forged-call' WHERE tool_name='agent_loop'",
                              "UPDATE desktop_trusted_apps SET capabilities='[\"observe\",\"fill\",\"interact\"]'"):
                    conn.execute(query)
                    conn.commit()
                    with self.assertRaises(RuntimeError):
                        manual.check_receipts(db, "rejected", "approved", executable, final=True)
                    conn.execute("UPDATE task_runs SET status='completed'")
                    conn.execute("DELETE FROM automation_runs")
                    conn.execute("DELETE FROM agent_steps WHERE call_id='replay'")
                    conn.execute("DELETE FROM agent_permission_grants")
                    conn.execute("UPDATE agent_steps SET call_id='agent-loop-00000000-0000-4000-8000-000000000001',tool_input='null',success=0 WHERE tool_name='agent_loop'")
                    conn.execute("UPDATE desktop_trusted_apps SET capabilities='[\"observe\",\"fill\"]'")
                    conn.commit()

    def test_desktop_cli_help_and_invalid_modes_never_launch_or_build(self):
        for arguments, expected in ((["--help"], 0), (["--reply-export", "--manual-native"], 2),
                                    (["--model-recovery", "--reply-export"], 2),
                                    (["--project-restore", "--manual-native"], 2),
                                    (["--project-restore", "--inspect"], 2), (["--inspect"], 2), (["--typo"], 2)):
            with self.subTest(arguments=arguments), patch.object(sys, "argv", ["desktop_e2e.py", *arguments]), \
                    patch.object(desktop_e2e, "prepare_desktop_binary") as build, \
                    patch.object(manual, "run_manual_native") as start, \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()), \
                    self.assertRaises(SystemExit) as exited:
                desktop_e2e.main()
            self.assertEqual(exited.exception.code, expected)
            build.assert_not_called()
            start.assert_not_called()

    def test_daily_attachment_script_allows_results_but_never_project_commands(self):
        captured = []

        def capture_script(_npx, env, _skip_build):
            captured.extend(json.loads(env["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"]))
            raise RuntimeError("owned script checkpoint")

        # Capture the real emitted fixture before any build, window or driver.
        with patch.object(sys, "argv", ["desktop_e2e.py", "--skip-build"]), \
                patch.object(desktop_e2e, "prepare_desktop_binary", side_effect=capture_script), \
                patch.object(desktop_e2e, "owned_fixture") as fixture, \
                patch.object(desktop_e2e, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "owned script checkpoint"):
                desktop_e2e.main()
            fixture.assert_not_called()
            run.assert_not_called()
        replies = [reply for reply in captured
                   if "daily-actions.csv" in reply.get("expectUserContentIncludes", [])]
        self.assertEqual(len(replies), 3, "initial, continued and edited snapshots must all be checked")
        for reply in replies:
            self.assertIn("attachment-e2e-marker", reply["expectUserContentIncludes"])
            self.assertEqual(reply["forbiddenToolNames"], ["run_project_command"],
                             "Personal owns result-file tools, not project command authority")

    def test_build_and_cache_never_use_normal_artifacts(self):
        startup = (REPO_ROOT / "src-tauri/src/lib.rs").read_text(encoding="utf-8")
        loader = startup.split("fn load_deployment_environment()", 1)[1].split("fn start_dev_http_server()", 1)[0]
        self.assertIn('#[cfg(not(any(test, feature = "desktop-e2e")))]', loader)
        self.assertEqual(startup.count("dotenvy::from_filename"), 1)
        self.assertEqual(startup.count("load_deployment_environment();"), 2)
        message_source = (REPO_ROOT / "src-tauri/src/commands/message.rs").read_text(encoding="utf-8")
        self.assertNotIn("generate_title_from_content", message_source)
        normal_config = json.loads((REPO_ROOT / "src-tauri/tauri.conf.json").read_text())
        # Tauri's native drop interception prevents HTML File-based drops on Windows.
        # Keep explicit user-selected bytes on the same bounded attachment path.
        self.assertIs(normal_config["app"]["windows"][0].get("dragDropEnabled"), False)
        config = json.loads((REPO_ROOT / "src-tauri/tauri.desktop-e2e.conf.json").read_text())
        # Windows known-folder resolution does not honor APPDATA environment
        # overrides for WebView2. Its default cache follows the app identifier.
        self.assertEqual(config.get("identifier"), "com.angelbot.desktop.e2e")
        self.assertNotEqual(config["identifier"], normal_config["identifier"])
        self.assertEqual(config["build"], {
            "frontendDist": "target/desktop-e2e/frontend",
            "beforeBuildCommand": "npm run build -- --outDir src-tauri/target/desktop-e2e/frontend",
        })
        for mode in ("build", "cached", "missing", "failed"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                normal = root / "src-tauri/target/debug/angelbot.exe"
                normal.parent.mkdir(parents=True)
                normal.write_bytes(b"daily desktop")
                normal_frontend = root / "dist/index.html"
                normal_frontend.parent.mkdir()
                normal_frontend.write_text("daily frontend")
                isolated = root / "src-tauri/target/desktop-e2e/debug/angelbot.exe"
                if mode == "cached":
                    isolated.parent.mkdir(parents=True)
                    isolated.write_bytes(b"cached desktop e2e")
                env = {
                    "CARGO_TARGET_DIR": str(normal.parent),
                    "ANGELBOT_DESKTOP_E2E_BINARY_PATH": str(normal),
                }

                def build(command, child_env):
                    self.assertEqual(child_env["CARGO_TARGET_DIR"], str(isolated.parents[1]))
                    self.assertEqual(child_env["ANGELBOT_DESKTOP_E2E_BINARY_PATH"], str(isolated))
                    self.assertEqual(command, [
                        "npx", "tauri", "build", "--debug", "--no-bundle",
                        "--features", "desktop-e2e", "--config",
                        "src-tauri/tauri.desktop-e2e.conf.json",
                    ])
                    if mode == "failed":
                        raise subprocess.CalledProcessError(2, command)
                    isolated.parent.mkdir(parents=True)
                    isolated.write_bytes(b"new desktop e2e")

                with patch.object(desktop_e2e, "ROOT", root), patch.object(desktop_e2e, "run", side_effect=build) as run:
                    if mode == "missing":
                        with self.assertRaisesRegex(RuntimeError, "without --skip-build"):
                            desktop_e2e.prepare_desktop_binary("npx", env, True)
                    elif mode == "failed":
                        with self.assertRaises(subprocess.CalledProcessError):
                            desktop_e2e.prepare_desktop_binary("npx", env, False)
                    else:
                        self.assertEqual(
                            desktop_e2e.prepare_desktop_binary("npx", env, mode == "cached"),
                            isolated,
                        )
                    self.assertEqual(run.call_count, int(mode in {"build", "failed"}))
                self.assertEqual(env["CARGO_TARGET_DIR"], str(isolated.parents[1]))
                self.assertEqual(env["ANGELBOT_DESKTOP_E2E_BINARY_PATH"], str(isolated))
                self.assertEqual(normal.read_bytes(), b"daily desktop")
                self.assertEqual(normal_frontend.read_text(), "daily frontend")

        with tempfile.TemporaryDirectory() as directory:
            env = {"ANGELBOT_DESKTOP_E2E_SEED_NETWORK_APPROVAL": "1", "ANGELBOT_DESKTOP_E2E_EXPLORER_SCRIPT": "unused"}
            desktop_e2e.configure_reply_export_probe(env, Path(directory))
            self.assertEqual(env["ANGELBOT_DESKTOP_E2E_REPLY_EXPORT"], "1")
            self.assertEqual(Path(env["ANGELBOT_DESKTOP_E2E_EXPORT_DIR"]), Path(directory) / "reply-exports")
            self.assertEqual((Path(directory) / "angelbot-data/results/download.bin").read_bytes(), bytes([0, 255, 1, 128]))
            self.assertEqual((Path(directory) / "angelbot-data/results/验收结果.csv").read_text(encoding="utf-8"), "事项,状态\n本机验收,完成\n")
            self.assertEqual(json.loads(env["ANGELBOT_DESKTOP_E2E_MODEL_SCRIPT"])[0]["type"], "text")
            self.assertNotIn("ANGELBOT_DESKTOP_E2E_SEED_NETWORK_APPROVAL", env)
            self.assertNotIn("ANGELBOT_DESKTOP_E2E_EXPLORER_SCRIPT", env)

        # Windows may reserve the driver's default 4445 (for example inside
        # 4404..4503). Both launch and polling must use one OS-selected port;
        # no OS exclusion/security settings are changed. Mock loopback here so
        # canonical full remains network-free.
        env = {"TAURI_WEBDRIVER_PORT": "4445", "UNCHANGED": "value"}
        with patch("socket.socket") as socket_factory:
            probe = socket_factory.return_value.__enter__.return_value
            probe.getsockname.return_value = ("127.0.0.1", 9254)
            desktop_e2e.configure_webdriver_port(env)
            probe.bind.assert_called_once_with(("127.0.0.1", 0))
            self.assertEqual(env, {"TAURI_WEBDRIVER_PORT": "9254", "UNCHANGED": "value"})
            self.assertTrue(socket_factory.return_value.__exit__.called)

    def test_desktop_visibility_requeries_replaced_streaming_nodes(self):
        node = shutil.which("node")
        self.assertIsNotNone(node, "Node is required by the existing desktop/frontend toolchain")
        script = """
          import assert from 'node:assert/strict';
          const {waitForVisible} = await import(process.argv[1]);
          let queries = 0, polls = 0;
          const options = {timeout: 20000, timeoutMsg: 'current reply must be visible'};
          const retired = {
            isDisplayed: async () => false,
            waitForDisplayed: async (supplied) => browser.waitUntil(() => retired.isDisplayed(), supplied),
          };
          const browser = {
            $: async (selector) => {
              assert.equal(selector, '.reply');
              return ++queries === 1 ? retired : {isDisplayed: async () => true};
            },
            waitUntil: async (condition, supplied) => {
              assert.deepEqual(supplied, options);
              for (let index = 0; index < 3; index++) {
                polls++;
                if (await condition()) return;
              }
              throw new Error('retired streaming element was polled instead of its rendered replacement');
            },
          };
          await waitForVisible(browser, '.reply', options);
          assert.equal(queries, 2);
          assert.equal(polls, 2);
          queries = 0;
          browser.$ = async () => { queries++; return retired; };
          await assert.rejects(() => waitForVisible(browser, '.reply', options), /retired streaming element/);
          assert.equal(queries, 3, 'unseen replacements must still time out');
        """
        result = subprocess.run(
            [node, "--input-type=module", "-e", script,
             (REPO_ROOT / "e2e/helpers/visibility.mjs").as_uri()],
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_wdio_requires_and_reuses_the_explicit_artifact(self):
        node = shutil.which("node")
        self.assertIsNotNone(node, "Node is required by the existing desktop/frontend toolchain")
        script = """
          const {config} = await import(process.argv[1]);
          console.log(JSON.stringify({paths: [
            config.capabilities[0]['tauri:options'].application,
            config.capabilities[0]['wdio:tauriServiceOptions'].appBinaryPath,
            config.services[0][1].appBinaryPath,
          ], specs: config.specs, timeout: config.mochaOpts.timeout}));
        """
        binary = REPO_ROOT / "src-tauri/target/desktop-e2e/debug/angelbot.exe"
        for supplied, probe in ((False, None), (True, None), (True, "reply-export"),
                                (True, "model-recovery"), (True, "project-restore")):
            with self.subTest(supplied=supplied, probe=probe):
                env = {name: os.environ[name] for name in ("PATH", "SYSTEMROOT") if name in os.environ}
                if supplied:
                    env["ANGELBOT_DESKTOP_E2E_BINARY_PATH"] = str(binary)
                if probe:
                    flag = {"reply-export": "REPLY_EXPORT", "model-recovery": "MODEL_RECOVERY",
                            "project-restore": "PROJECT_RESTORE"}[probe]
                    env[f"ANGELBOT_DESKTOP_E2E_{flag}"] = "source" if probe == "project-restore" else "1"
                result = subprocess.run(
                    [node, "--input-type=module", "-e", script, (REPO_ROOT / "e2e/wdio.conf.mjs").as_uri()],
                    env=env, capture_output=True, text=True, timeout=10,
                )
                if supplied:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    configuration = json.loads(result.stdout)
                    self.assertEqual(configuration["paths"], [str(binary)] * 3)
                    self.assertEqual(configuration["specs"], [f"./specs/{probe or 'first-run'}.e2e.mjs"])
                    self.assertEqual(configuration["timeout"], 600_000 if probe == "project-restore" else 300_000 if probe else 90_000)
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("isolated binary path is required", result.stderr)

        # Execute the real spec's failure lifecycle without a desktop. It must
        # invalidate old success evidence and seal the checkpoint, not emit raw
        # diagnostics or fabricate a passed native-picker journey.
        with tempfile.TemporaryDirectory(prefix="angelbot-desktop-e2e-") as directory:
            root = Path(directory)
            evidence = root / "src-tauri/target/e2e"
            evidence.mkdir(parents=True)
            (evidence / "reply-export-desktop.json").write_text('{"status":"passed"}')
            exports = root / "reply-exports"
            exports.mkdir()
            script = """
              import {readFileSync} from 'node:fs';
              import {join} from 'node:path';
              process.chdir(process.argv[2]);
              process.env.ANGELBOT_DESKTOP_E2E_EXPORT_DIR = join(process.cwd(), 'reply-exports');
              let body, cleanup;
              globalThis.describe = (_, fn) => fn();
              globalThis.it = (_, fn) => { body = fn; };
              globalThis.afterEach = fn => { cleanup = fn; };
              globalThis.browser = {tauri: {execute: async () => { throw new Error('PRIVATE_NATIVE_ERROR'); }}};
              await import(process.argv[1]);
              try { await body(); throw new Error('expected failure'); }
              catch (error) { if (error.message !== 'PRIVATE_NATIVE_ERROR') throw error; }
              cleanup.call({currentTest: {state: 'failed'}});
              console.log(readFileSync('src-tauri/target/e2e/reply-export-desktop.json', 'utf8'));
            """
            result = subprocess.run([node, "--input-type=module", "-e", script,
                (REPO_ROOT / "e2e/specs/reply-export.e2e.mjs").as_uri(), str(root)],
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads(result.stdout)
            self.assertEqual(report["status"], "failed")
            self.assertEqual(report["phase"], "startup")
            self.assertFalse(any(report[key] for key in ("nativeCancellation", "markdownExact", "textExact", "transcriptUnchanged")))
            self.assertNotIn("PRIVATE_NATIVE_ERROR", result.stdout)
            self.assertEqual(json.loads((exports / "stage.json").read_text()), {"phase": "failed", "failedPhase": "startup"})

        # A build/driver failure occurs before the JS spec's afterEach hook.
        # It must also invalidate a previous pass, without logging diagnostics
        # into the acceptance report.
        for failure in ("build", "driver", "incomplete", "none"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                report_path = root / "src-tauri/target/e2e/reply-export-desktop.json"
                report_path.parent.mkdir(parents=True)
                report_path.write_text('{"status":"passed"}')
                error = subprocess.CalledProcessError(2, "PRIVATE_STARTUP_ERROR")
                def driver(command, env):
                    if failure == "driver":
                        raise error
                    if failure == "none":
                        report_path.write_text(json.dumps({"status": "passed", "phase": "done",
                            "nativeCancellation": True, "markdownExact": True,
                            "textExact": True, "transcriptUnchanged": True, "dailyPreviewExact": True,
                            "dailyNativeCancellation": True, "binaryExportExact": True}))

                with patch.object(desktop_e2e, "ROOT", root), \
                        patch.object(desktop_e2e, "prepare_desktop_binary", side_effect=error if failure == "build" else None), \
                        patch.object(desktop_e2e, "configure_webdriver_port"), \
                        patch.object(desktop_e2e, "run", side_effect=driver):
                    if failure == "none":
                        desktop_e2e.run_reply_export_probe("npx", {}, root, False)
                    else:
                        with self.assertRaises(RuntimeError if failure == "incomplete" else subprocess.CalledProcessError):
                            desktop_e2e.run_reply_export_probe("npx", {}, root, False)
                report = json.loads(report_path.read_text())
                self.assertEqual(report["status"], "passed" if failure == "none" else "failed")
                self.assertEqual(all(report[key] for key in ("nativeCancellation", "markdownExact", "textExact", "transcriptUnchanged")), failure == "none")
                self.assertNotIn("PRIVATE_STARTUP_ERROR", report_path.read_text())


if __name__ == "__main__":
    unittest.main()
