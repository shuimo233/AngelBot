#!/usr/bin/env python3
"""THROWAWAY: can one query-only Luna call repair empty lexical preference lookup?

python scripts/prototypes/query_expansion_prototype.py --replay        # no credentials/network
python scripts/prototypes/query_expansion_prototype.py --replay --live # <=4 calls, no Main/app
Add --context-ablation for the separate frozen paired-intent experiment.
Default is an offline one-shot TUI. Corpus is manually prepared in scratch memory,
not learned by a model; this screens retrieval, never companionship/Main accuracy.
"""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from context_eval import ROOT, run_context_worker

WORKER='commands::message::live_tests::query_expansion_prototype::configured_query_expansion_prototype'
PROMPT='你只负责为个人助手的本地关键词检索生成替代表达，不能回答用户问题或推断已保存内容。索引以中文为主。给出与原查询同一主题的1到5个简短搜索词，优先保留核心主题，可含中文翻译和常用同义表达，避免泛化到不相关主题。每个词2到24个字符，不能原样重复查询。只返回JSON对象 {"queries":["词1","词2"]}，不要Markdown或额外字段，不得调用工具。'
RECORDS=[{'key':'drink','content':'以后饮品优先选择绿茶。'},
         {'key':'nickname','content':'聊天时用昵称小宁称呼我，不用正式头衔。'},
         {'key':'closing','content':'每个回答的末尾都另起一行写：别急，慢慢来。'}]
CASES=[{'id':'exact_control','query':'绿茶','expected':['drink']},
       {'id':'cross_language_drink','query':'green tea','expected':['drink']},
       {'id':'cross_language_name','query':'nickname','expected':['nickname']},
       {'id':'closing_concept','query':'sign-off convention','expected':['closing']},
       {'id':'unrelated_finance','query':'bank loans','expected':[]}]
CONTEXT_RECORDS=[{'key':'tone','content':'以后交流别太官方，用自然一点的语气。'},
                 {'key':'length','content':'回答尽量短，细节需要时我再问。'},
                 {'key':'notes','content':'重要的事情我习惯登记到纸质笔记本上。'}]
CONTEXT_CASES=[{'id':'register_query_only','query':'register','expected':['tone']},
               {'id':'register_with_intent','query':'register','context':'我想找回以前对聊天正式程度的偏好。','expected':['tone']},
               {'id':'brief_query_only','query':'brief','expected':['length']},
               {'id':'brief_with_intent','query':'brief','context':'我想查以前对回答篇幅的偏好。','expected':['length']}]


def fixture(context_ablation):
    return (CONTEXT_RECORDS,CONTEXT_CASES) if context_ablation else (RECORDS,CASES)


def request_text(case):
    return json.dumps({'query':case['query'],'task_context':case['context']},ensure_ascii=False,separators=(',',':')) if case.get('context') else case['query']


def projection(live,context_ablation=False):
    records,cases=fixture(context_ablation)
    return {'version':1,'live':live,'prompt':PROMPT,'records':[dict(r) for r in records],
            'cases':[{k:c[k] for k in ('id','query','context') if k in c} for c in cases]}


def verified_hints(row, original):
    if not row.get('requestCount',0):return row.get('queries',[])==[]
    outputs=row.get('providerOutputs',[])
    if len(outputs)!=1 or outputs[0].get('kind')!='text' or outputs[0].get('stopReason')!='EndTurn' or outputs[0].get('truncated') is not False:return False
    try:
        payload=json.loads(outputs[0]['text']);raw=payload['queries']
        if set(payload)!={'queries'} or not isinstance(raw,list) or not 1<=len(raw)<=5:return False
        result=[];seen={original.strip().lower()}
        for value in raw:
            if not isinstance(value,str) or not 2<=len(value.strip())<=24:return False
            if value.strip().lower() not in seen:
                result.append(value.strip());seen.add(value.strip().lower())
        return bool(result) and result==row.get('queries')
    except (ValueError,KeyError,TypeError):return False


def assess(rows, live,context_ablation=False):
    records,cases=fixture(context_ablation)
    by_id={r['id']:r for r in rows}
    checks={}
    for case in cases:
        row=by_id.get(case['id'],{})
        frames=row.get('providerInputs',[]);calls=row.get('requestCount',0)
        baseline=row.get('baseline',[])
        expected=set(case['expected'])
        observed=[r for field in ('baseline','final','localContext','rejectedPartial') for r in row.get(field,[])]
        valid=all(r.get('key') in {record['key'] for record in records} and
                  r.get('quote')==next(record['content'] for record in records if record['key']==r['key']) for r in observed)
        checks[case['id']]={'executed':bool(row),'baselineExact':{r['key'] for r in baseline}==expected,
            'finished':row.get('status') in (('skipped_hit','completed') if live else ('skipped_hit','would_expand')),
            'onlyDeclaredInputSent':len(frames)==calls and all(f.get('tools') is None and
                [(m.get('role'),m.get('content')) for m in f.get('messages',[])]==[('system',PROMPT),('user',request_text(case))] for f in frames),
            'conditionalBudget':calls==(int(live and not baseline)) if row else False,
            'hintReceipt':verified_hints(row,case['query']),
            'sourceUnchanged':row.get('storeUnchanged') is True,'originalQuotesOnly':valid,
            'expandedExact':row.get('status') in ('skipped_hit','completed') and {r['key'] for r in row.get('final',[])}==expected if live else None,
            'localContextDirectExact':row.get('localContextPartial') is False and {r['key'] for r in row.get('localContext',[])}==expected if case.get('context') else None}
    complete=len(rows)==len(cases) and len(by_id)==len(rows)
    models={r.get('model') for r in rows if r.get('requestCount',0)}
    same_model=not live or len(models)==1 and all(isinstance(m,str) and 'luna' in m for m in models)
    safe=complete and same_model and sum(r.get('requestCount',0) for r in rows)<=4 and all(all(v for k,v in c.items() if k not in ('baselineExact','expandedExact','localContextDirectExact')) for c in checks.values())
    status=('passed' if all(c['expandedExact'] for c in checks.values()) else 'needs_work') if safe and live else ('offline_only' if safe else 'blocked')
    return status,checks


def run(live=False,context_ablation=False):
    records,cases=fixture(context_ablation)
    owned=Path(tempfile.mkdtemp(prefix='query-expansion-prototype-',dir=ROOT/'logs'))
    report={'status':'building','checkedAt':datetime.now(timezone.utc).isoformat(),'condition':'query_expansion_context_ablation_v2' if context_ablation else 'query_only_expansion_prototype_v1',
            'officialBenchmark':False,'live':live,'maxProviderCalls':4 if live else 0,'plannedCases':len(cases),
            'fixture':{'records':records,'cases':cases},'limits':['Manual scratch records; no model learning or actual Main answer',
                'One query-only expansion after empty lookup; no tools, original preferences or rubric sent to model',
                'Expanded partial-character candidates are recorded but rejected, never treated as direct source evidence',
                'No ordinary data, app restart, background service, embedding or paid-API fallback',
                'Context ablation uses new queries/corpus, same system prompt and paired query-only vs short neutral task intent; local context lookup is a zero-call shadow control'],
            'sourceSha256':{p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in (
                'scripts/prototypes/query_expansion_prototype.py','src-tauri/src/commands/message_live_tests/query_expansion_prototype.rs',
                'src-tauri/src/agent/support/adaptive_constraints.rs','src-tauri/src/fts.rs',
                'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs','scripts/context_eval.py')}}
    target=owned/'acceptance.json'
    print(f'QUERY_EXPANSION_REPORT {target}',flush=True)
    (owned/'input.json').write_text(json.dumps(projection(live,context_ablation),ensure_ascii=False),encoding='utf-8')
    report['usageIncomplete']=True
    try:
        result,rows=run_context_worker(owned,WORKER,'QUERY_EXPANSION_ROW ')
        report['status'],report['checks']=assess(rows,live,context_ablation)
        report.update(results=rows,exitCode=result.returncode,usage={k:sum(r.get(k,0) for r in rows) for k in ('requestCount','inputTokens','outputTokens')})
        report['usageIncomplete']=result.returncode!=0 or any(o.get('kind')=='provider_error' for r in rows for o in r.get('providerOutputs',[]))
        if result.returncode:report['status']='blocked'
    except BaseException:
        report['status']='blocked'
        raise
    finally:
        target.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--replay',action='store_true')
    parser.add_argument('--live',action='store_true')
    parser.add_argument('--context-ablation',action='store_true',help='new frozen paired questions; same prompt, <=4 calls')
    args=parser.parse_args()
    records,cases=fixture(args.context_ablation)
    if args.replay:
        report=run(args.live,args.context_ablation)
        print(json.dumps({'status':report['status'],'usage':report.get('usage'),'checks':report.get('checks')},ensure_ascii=False))
        return 0 if report['status'] in ('offline_only','passed') else 2
    index=0;report=None
    while True:
        print('\x1b[2J\x1b[H',end='')
        observed=next((r for r in report.get('results',[]) if r['id']==cases[index]['id']),{}) if report else {}
        state={'prototype':'query expansion','live':args.live,'case':cases[index],
               'records':{r['key']:r['content'] for r in records},
               'queries':observed.get('queries'),
               'baseline':[r['key'] for r in observed.get('baseline',[])],
               'final':[r['key'] for r in observed.get('final',[])],
               'rejectedPartial':[r['key'] for r in observed.get('rejectedPartial',[])],
               'result':report.get('checks',{}).get(cases[index]['id']) if report else 'not run',
               'usage':report.get('usage') if report else None}
        print(json.dumps(state,ensure_ascii=False,indent=2))
        print('[r] run batch once  [n] next case  [q] quit')
        action=input('> ').strip().lower()
        if action=='q':return 0
        if action=='n':index=(index+1)%len(cases)
        if action=='r' and report is None:report=run(args.live,args.context_ablation)


if __name__=='__main__':
    sys.stdout.reconfigure(encoding='utf-8')
    raise SystemExit(main())
