#!/usr/bin/env python3
"""THROWAWAY combined memory-core comparison, not a production framework.

python scripts/prototypes/minimal_architecture.py --replay        # describe only
python scripts/prototypes/minimal_architecture.py --replay --live # one bounded batch
Add --sequence for the separate frozen v2: correction, exception review and scope;
compares 12/6 initial records, not a change to production configuration.
Default is an offline TUI. No app, desktop, business tools or ordinary DB writes.
Gold stays here; the Rust worker receives only original sources and questions.
"""
from __future__ import annotations

import argparse
import contextlib
from copy import deepcopy
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from context_eval import ROOT, run_context_worker

WORKER = 'commands::message::live_tests::minimal_architecture::configured_minimal_architecture'
ARMS = ('full_snapshot', 'bounded_recall')
SOURCES = ['以后聊天称呼我为小沐，不用职务。',
           '以后每条回复的最后，单独一行写“记得歇一歇。”',
           '我想换个结尾，从今后改成“今天也辛苦了。”']
TOPICS = ('旅行 清洁 收纳 阅读 绘画 摄影 烘焙 园艺 徒步 跑步 骑行 游泳 瑜伽 钓鱼 露营 '
          '观鸟 编织 陶艺 雕刻 插花 书法 象棋 围棋 桌游 魔方 拼图 手帐 集邮 乐器 唱歌 '
          '舞蹈 天文 地理 历史 建筑 手工').split()
NOISE = [f'以后介绍{topic}相关知识时，先给一个简短例子，再列出两点注意事项。' for topic in TOPICS]
CASES = [
    {'id': 'address', 'question': '我们以前约定你怎么称呼我？只写称呼本身；没有可核对记录就只写“未确认”。', 'expected': '小沐', 'source_needed': True},
    {'id': 'exception', 'question': '这次请用英文回答，只返回两个词：All set。不要称呼或固定收尾句。', 'expected': 'All set', 'source_needed': False},
    {'id': 'latest_closing', 'question': '我们长期约定的最新固定收尾句是什么？只写句子本身；没有可核对记录就只写“未确认”。', 'expected': '今天也辛苦了。', 'source_needed': True},
    {'id': 'unknown', 'question': '我之前告诉过你我正在用的咖啡机的具体型号吗？只写型号；没有可核对记录就只写“未确认”。', 'expected': '未确认', 'source_needed': False},
]
SEQUENCE_ARMS = ('view12', 'view6')
SEQUENCE = [
    {'id': 'initial', 'sources': ['以后聊天称呼我小榆。', '以后推荐饮品时优先推荐乌龙茶。'],
     'probes': [{'id': 'initial_address', 'question': '我们约定你怎么称呼我？只写称呼本身。', 'expected': '小榆'}]},
    {'id': 'correction', 'sources': ['我改主意了，往后给我推荐饮品时优先推荐白茶。'],
     'probes': [{'id': 'corrected_drink', 'question': '按我平时的喜好推荐一种下午喝的饮品，只写饮品名称。', 'expected': '白茶'}]},
    {'id': 'exception', 'sources': ['这一次推荐饮品时请选乌龙茶，只写名称。'],
     'probes': [{'id': 'one_off_drink', 'question': '这一次推荐饮品时请选乌龙茶，只写名称。', 'expected': '乌龙茶'}]},
    {'id': 'reopen', 'sources': [],
     'probes': [{'id': 'default_after_exception', 'question': '按我平时的喜好推荐一种下午喝的饮品，只写饮品名称。', 'expected': '白茶'},
                {'id': 'old_address', 'question': '我们之前约定你怎么称呼我？只写称呼本身。', 'expected': '小榆', 'source_needed': True}]},
    {'id': 'conditional', 'sources': ['以后到晚上再推荐饮品时，优先推荐温水；白天继续按原来的饮品偏好。'],
     'probes': [{'id': 'night_drink', 'question': '按我的喜好，晚上推荐一种饮品，只写名称。', 'expected': '温水'},
                {'id': 'day_drink', 'question': '按我的喜好，白天推荐一种饮品，只写名称。', 'expected': '白茶'}]},
]
TRANSFER = [
    {'id': 'initial', 'sources': ['以后给我安排日常通勤，默认坐地铁。',
                                '以后向我汇报事项，先给详细过程，再给结论。'], 'probes': []},
    {'id': 'correction', 'sources': ['我换习惯了，往后日常通勤默认骑自行车，不再默认坐地铁。',
                                   '以后汇报先给结论，详细过程等我需要时再展开。'], 'probes': []},
    {'id': 'exception', 'sources': ['这次通勤安排选地铁，仅限今天。',
                                  '这条汇报先写过程就好，不要沿用到以后。'], 'probes': []},
    {'id': 'conditional', 'sources': ['以后下雨天通勤默认打车；不下雨仍按原来通勤偏好。',
                                    '汇报涉及安全风险时，先明确风险再给结论；其他事项照原来的顺序。'],
     'probes': [
         {'id': 'commute_current', 'question': '明天不下雨，按我长期通勤习惯该怎么出行？记得以前提过地铁，请查记录后只给现在的交通方式；证据不足只写未确认。', 'expected': '自行车', 'accepted': ['自行车', '骑自行车'], 'readback': '自行车'},
         {'id': 'commute_rain', 'question': '我记得过去通勤常选地铁。假如下雨，按我后来留下的约定现在该怎么走？查记录后只写方式，无依据答未确认。', 'expected': '打车', 'readback': '打车'},
         {'id': 'commute_history', 'question': '查一下最早保存的通勤约定，只告诉我当时默认的交通方式，不要用后来的更正覆盖历史。仅写名称。', 'expected': '地铁', 'accepted': ['地铁', '坐地铁'], 'readback': '地铁'},
         {'id': 'report_default', 'question': '普通事项汇报应先讲过程还是先讲结论？请核对我长期约定的最新顺序，别把一次例外当默认。只输出过程或结论，依据不足输出未确认。', 'expected': '结论', 'readback': '结论'},
         {'id': 'report_safety', 'question': '我要听一项涉及安全风险的事项汇报。按我保存的偏好，最先应讲什么？查记录后只回复风险、过程或结论；没有依据答未确认。', 'expected': '风险', 'readback': '安全风险'},
         {'id': 'commute_unknown', 'question': '我给通勤自行车指定过哪个品牌？查记忆后只给明确记录的品牌名，没有就只写未确认。', 'expected': '未确认'},
         {'id': 'exclude_preferences', 'question': '只允许查询非偏好的事实记忆，不要查询或应用任何偏好。事实记忆里记录了我的通勤方式吗？只给方式；没有事实依据就只写未确认。', 'expected': '未确认', 'exclude_preferences': True},
     ]},
]
NATURAL_CASES = [
    {'id': 'natural_sunny', 'question': '明天晴天，我照常去上班，你优先建议哪种交通方式？只写方式；不确定写未确认。', 'expected': '自行车', 'accepted': ['自行车', '骑自行车'], 'readback': '自行车'},
    {'id': 'natural_rain', 'question': '明早有雨，我怎么去上班合适？只写方式；不确定写未确认。', 'expected': '打车', 'readback': '打车'},
    {'id': 'natural_report', 'question': '周报材料已齐全、没有风险项。请按我平时习惯，指出开头先放过程还是结论，只答一个词；无法确定答未确认。', 'expected': '结论', 'readback': '结论'},
]
COMMUTE_TODAY = '今天不下雨，按我的约定优先怎么去上班？只写方式，没有依据只写未确认。'
COMMUTE_RAIN = '今天下雨，按我平时的约定优先怎么去上班？只写方式，没有依据只写未确认。'
TEMPORAL = [
    {'id': 'future', 'evidence_terms': ['2026年11月1日', '公交车'], 'sources': ['从北京时间2026年11月1日零点起，不下雨时的日常通勤改为坐公交车；这之前仍骑自行车，下雨仍然打车。'],
     'probes': [
         {'id': 'before_start', 'question': COMMUTE_TODAY, 'clock': '2026-10-31T23:59:00+08:00', 'expected': '自行车', 'accepted': ['自行车', '骑自行车'], 'source_stage': 'future'},
         {'id': 'at_start', 'question': COMMUTE_TODAY, 'clock': '2026-11-01T00:00:00+08:00', 'expected': '公交车', 'accepted': ['公交车', '公交', '坐公交车'], 'source_stage': 'future'},
         {'id': 'future_rain', 'question': COMMUTE_RAIN, 'clock': '2026-11-02T12:00:00+08:00', 'expected': '打车', 'source_stage': 'future'},
     ]},
    {'id': 'withdraw', 'evidence_terms': ['取消', '公交车'], 'sources': ['取消之前定于11月1日起改坐公交车的安排；今后不下雨仍优先骑自行车，下雨仍优先打车。'],
     'probes': [
         {'id': 'withdraw_default', 'question': COMMUTE_TODAY, 'clock': '2026-11-01T00:00:00+08:00', 'expected': '自行车', 'accepted': ['自行车', '骑自行车'], 'source_stage': 'withdraw'},
         {'id': 'withdraw_history', 'question': '最初提出从11月1日调整通勤的那条安排，原计划选什么方式？只回顾当时计划，不问现行规则；只写方式，无依据写未确认。', 'clock': '2026-11-01T00:00:00+08:00', 'expected': '公交车', 'accepted': ['公交车', '公交', '坐公交车'], 'source_stage': 'future'},
         {'id': 'withdraw_rain', 'question': COMMUTE_RAIN, 'clock': '2026-11-02T12:00:00+08:00', 'expected': '打车', 'source_stage': 'withdraw'},
     ]},
]
TEMPORAL_FOLLOWUP = [
    {'id': 'future', 'evidence_terms': ['2026年11月5日', '步行'], 'sources': ['从北京时间2026年11月5日18点30分开始，无雨的上班出行优先步行；那之前继续骑自行车，遇上下雨照旧打车。', '今天这趟去公司先坐地铁，仅限这一次。'],
     'probes': [
         {'id': 'before_start_new', 'question': '今天没下雨，照我的约定，去公司应优先用哪种方式？只给方式，依据不足写未确认。', 'clock': '2026-11-05T18:29:00+08:00', 'expected': '自行车', 'accepted': ['自行车', '骑自行车'], 'source_stage': 'future'},
         {'id': 'at_start_new', 'question': '今天没下雨，照我的约定，去公司应优先用哪种方式？只给方式，依据不足写未确认。', 'clock': '2026-11-05T18:30:00+08:00', 'expected': '步行', 'accepted': ['步行', '走路'], 'source_stage': 'future'},
         {'id': 'future_rain_new', 'question': '今天下雨，上班我通常该优先怎么出行？只给方式，依据不足写未确认。', 'clock': '2026-11-06T12:00:00+08:00', 'expected': '打车', 'source_stage': 'future'},
     ]},
    {'id': 'withdraw', 'evidence_terms': ['撤回', '步行'], 'sources': ['我撤回从11月5日傍晚起改成步行的那个长期约定；日后无雨上班继续优先骑自行车，雨天继续打车。'],
     'probes': [
         {'id': 'withdraw_default_new', 'question': '今天没下雨，照我的约定，去公司应优先用哪种方式？只给方式，依据不足写未确认。', 'clock': '2026-11-05T18:30:00+08:00', 'expected': '自行车', 'accepted': ['自行车', '骑自行车'], 'source_stage': 'withdraw'},
         {'id': 'withdraw_history_new', 'question': '那条原先约好从11月5日傍晚开始的出行方式是什么？只回顾原先约定，不问现在是否撤回；只写方式，依据不足写未确认。', 'clock': '2026-11-05T18:30:00+08:00', 'expected': '步行', 'accepted': ['步行', '走路'], 'source_stage': 'future'},
         {'id': 'withdraw_rain_new', 'question': '今天下雨，上班我通常该优先怎么出行？只给方式，依据不足写未确认。', 'clock': '2026-11-06T12:00:00+08:00', 'expected': '打车', 'source_stage': 'withdraw'},
     ]},
]


def budget_stages():
    current = '如果我现在立刻出发、外面不下雨，按此刻已生效的约定，应优先怎样去公司？只写方式，无依据写未确认。'
    rain = '我现在立刻要去上班，外面下雨。按目前仍有效的偏好，应优先选哪种方式？只写方式，无依据写未确认。'
    questions = (current, current, rain, current,
                 '只回顾最初那条从11月5日傍晚起调整出行的约定，原先计划采用哪种方式？不问现行规则；只写方式，无依据写未确认。',
                 '截至现在，我明确指定的通勤自行车品牌是什么？只写能核对的品牌名，无记录写未确认。')
    stages, index = [], 0
    for stage in TEMPORAL_FOLLOWUP:
        probes = []
        for case in stage['probes']:
            probes.append(dict(case, id='budget_'+case['id'], question=questions[index]))
            index += 1
        stages.append(dict(stage, sources=list(stage['sources']), probes=probes))
    stages[-1]['probes'][-1] = dict(id='budget_unknown_brand', question=questions[-1],
        clock='2026-11-06T12:00:00+08:00', expected='未确认', lookup_required=True)
    return stages


BUDGET = budget_stages()
STABILITY_NOW = '我现在就出门去公司，天晴。按照此时适用的长期约定，只写交通方式；没有记录写未确认。'
# Frozen fresh wording on existing learned topics; two trials are repeated
# observations of these six cases, not twelve independent user samples.
STABILITY = [
    dict(id='stable_before', question=STABILITY_NOW, prefix=0, clock='2026-11-05T18:29:00+08:00',
         expected='自行车', accepted=['自行车', '骑自行车'], source_prefix=0),
    dict(id='stable_at', question=STABILITY_NOW, prefix=0, clock='2026-11-05T18:30:00+08:00',
         expected='步行', accepted=['步行', '走路'], source_prefix=0),
    dict(id='stable_withdraw', question='出发前再确认一下：没有下雨，现在通勤该用哪种方式？只写方式，记录不足写未确认。',
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='自行车', accepted=['自行车', '骑自行车'], source_prefix=1),
    dict(id='stable_history', question='不问今天的建议，只核对当初对11月5日傍晚之后的出行计划：那时约好改用什么？只写方式，记录不足写未确认。',
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='步行', accepted=['步行', '走路'], source_prefix=0),
    dict(id='stable_exception', question='这一次我明确选择坐地铁去公司，仅这一趟。请只复述本次交通方式，不沿用长期默认。',
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='地铁', accepted=['地铁', '坐地铁']),
    dict(id='stable_unknown', question='之前我有没有明确选定通勤自行车的厂商品牌？只给已记录的品牌，没有明确记录写未确认。',
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='未确认', lookup_required=True),
]
# A separate diagnostic set: make the weather/time branch explicit without
# providing the stored answer. Old v11/v12 questions and scores stay frozen.
CONDITIONS = [
    dict(id='condition_history', question='原计划在11月5日18点30分以后，无雨上班时优先怎么出行？只回顾当初那条计划，不问现在是否执行。只写方式；没有原记录写未确认。',
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='步行', accepted=['步行', '走路'], source_prefix=0),
    dict(id='condition_current', question='现在无雨，我马上出门上班。请按后来更正后的现行长期约定，只给交通方式；记录不足写未确认。',
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='自行车', accepted=['自行车', '骑自行车'], source_prefix=1),
    dict(id='condition_rain', question='现在下雨，我马上出门上班。请按后来更正后的现行长期约定，只给交通方式；记录不足写未确认。',
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='打车', accepted=['打车', '坐出租车', '出租车'], source_prefix=1),
]

# Task application rather than an explicit request to remember a preference.
# Values and scoring stay here; the worker receives only the task and clock.
DAILY_CARD = '只输出一张JSON小卡片，格式为{"方式":"交通方式","说明":"一句出门前提醒"}。'
DAILY = [
    dict(id='daily_current', question='我现在要去公司，外面没有下雨。帮我安排这趟出行。'+DAILY_CARD,
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='自行车', accepted=['自行车', '骑自行车'], source_prefix=1, structured=True),
    dict(id='daily_rain', question='我现在要去公司，外面正在下雨。帮我安排这趟出行。'+DAILY_CARD,
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='打车', accepted=['打车', '出租车', '坐出租车', '网约车'], source_prefix=1, structured=True),
    dict(id='daily_history', question='我在整理旧计划：当初定于11月5日18点30分之后、无雨上班的方案是什么？填一张JSON归档卡片，字段为方式、说明；说明写当初的安排，不填现在的建议。只输出JSON。',
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='步行', accepted=['步行', '走路'], source_prefix=0, structured=True),
    dict(id='daily_once', question='今天仅这一趟，我就坐地铁去公司。帮我填出行卡片。'+DAILY_CARD,
         prefix=1, clock='2026-11-06T12:00:00+08:00', expected='地铁', accepted=['地铁', '坐地铁', '乘地铁', '乘坐地铁'], structured=True),
]

# Boundary regression for the completed-lookup contract. Old questions/gold stay
# frozen; these cards avoid conflating an activity label with prose formatting.
GUARD_CARD = '只输出JSON：{"选择":"答案名称","说明":"一句依据"}；记录不足时选择填未确认。'
GUARDS = [dict(base,id=base['id'].replace('stable_','guard_'),question=question+GUARD_CARD,
               structured=True,choice_key='选择') for base,question in zip(STABILITY,[
    '我现在无雨出门上班。请按此时适用的长期约定填出行卡。',
    '我现在无雨出门上班。请按此时适用的长期约定填出行卡。',
    '现在无雨，马上出门上班。按后来更正后的现行长期约定填出行卡。',
    '回顾当初定于11月5日18点30分以后、无雨上班的出行计划；不问现行安排。',
    '这次我明确选择坐地铁去公司，仅限这一趟；请填本次交通方式，不沿用长期默认。',
    '以前有没有明确选定通勤自行车的厂商品牌？请查已保存资料，填有依据的品牌名。',
])]
GUARDS[4]['accepted'] = DAILY[3]['accepted']

# Two manually nominated profiles over the same archived background. These
# declarations are controlled inputs, not model-learned samples or a new policy.
COUNTER_SOURCES = [
    dict(id='counter_rain', quotes=['以后小雨上班优先打车，晴天照旧。', '以后小雨上班优先地铁，晴天照旧。']),
    dict(id='counter_tea', quotes=['以后下午茶的饮品优先乌龙茶。', '以后下午茶的饮品优先白茶。']),
]
COUNTER = [dict(id=f'{source["id"]}_{prefix}', question=question, prefix=prefix,
    clock='2026-11-07T12:00:00+08:00', expected=value, accepted=accepted, source_prefix=prefix,
    structured=True, choice_key='选择')
    for source, question, values, aliases in [
        (COUNTER_SOURCES[0], '我现在要去公司，外面下小雨。帮我安排这趟出行，只输出JSON：{"选择":"交通方式名称","说明":"一句准备建议"}。',
         ['打车','地铁'], [['打车','出租车','坐出租车','网约车'],['地铁','坐地铁','乘地铁','乘坐地铁']]),
        (COUNTER_SOURCES[1], '下午休息时想喝点茶，帮我选一种，只输出JSON：{"选择":"饮品名称","说明":"一句准备建议"}。',
         ['乌龙茶','白茶'], [['乌龙茶'],['白茶']]),
    ] for prefix, (value, accepted) in enumerate(zip(values, aliases))]

# New topics for a paired window-size screen, not another run of the v17 tasks.
WINDOW_SOURCES = [
    dict(id='window_shirt', quotes=['以后工作日挑衬衫，颜色优先深蓝色。', '以后工作日挑衬衫，颜色优先浅灰色。']),
    dict(id='window_relax', quotes=['以后下班后放松，优先听爵士乐。', '以后下班后放松，优先看纪录片。']),
]
WINDOW_CASES = [dict(id=f'{source["id"]}_{prefix}', question=question, prefix=prefix,
    clock='2026-11-07T12:00:00+08:00', expected=value, accepted=accepted, source_prefix=prefix,
    structured=True, choice_key='选择')
    for source, question, values, aliases in [
        (WINDOW_SOURCES[0], '帮我选明天工作日穿的衬衫颜色，只输出JSON：{"选择":"颜色名称","说明":"一句搭配建议"}。',
         ['深蓝色','浅灰色'], [['深蓝色','深蓝'],['浅灰色','浅灰']]),
        (WINDOW_SOURCES[1], '今天忙完下班了，帮我安排一个放松活动，只输出JSON：{"选择":"活动名称","说明":"一句准备建议"}。',
         ['听爵士乐','看纪录片'], [['听爵士乐','爵士乐','听爵士音乐'],['看纪录片','观看纪录片','纪录片']]),
    ] for prefix, (value, accepted) in enumerate(zip(values, aliases))]


def stability_cases(sources=False, scenario='original'):
    if scenario not in ('original', 'conditions', 'daily', 'counter', 'counter-view', 'view-budget', 'lookup-guards') or scenario != 'original' and not sources:
        raise ValueError('diagnostic scenarios require the existing source-reader surface')
    return GUARDS if scenario == 'lookup-guards' else WINDOW_CASES if scenario == 'view-budget' else COUNTER if scenario in ('counter','counter-view') else DAILY if scenario == 'daily' else CONDITIONS if scenario == 'conditions' else STABILITY[2:4] if sources else STABILITY


STALE_CASES = [
    {'id': 'current_day_old_term', 'question': '先查我保存过的“乌龙茶”相关偏好。它现在还是我白天默认的饮品吗？没改就只写乌龙茶，改过就只写目前默认的饮品名；证据不足只写未确认。', 'expected': '白茶', 'readback': '白茶'},
    {'id': 'historical_old_term', 'question': '查一下以前保存的“乌龙茶”约定。这次只回顾最早那条饮品偏好，不问当前偏好；只写最早那种饮品的名称。', 'expected': '乌龙茶', 'readback': '乌龙茶'},
    {'id': 'current_night_old_term', 'question': '先查我保存过的“乌龙茶”相关偏好。现在晚上也应照这条推荐吗？只写当前夜间优先的饮品名；不能核对就只写未确认。', 'expected': '温水', 'readback': '温水'},
    {'id': 'unknown_brand_old_term', 'question': '查我保存过的“乌龙茶”相关偏好，我有没有说具体买哪个品牌？只写品牌名称，没有原记录支持就只写未确认。', 'expected': '未确认'},
]
STALE_FOLLOWUP = [
    {'id': 'replaced_day_choice', 'question': '我印象里你会优先推荐乌龙茶。请查过记录后告诉我，白天现在该优先推荐什么？只给饮品名，记录不足答未确认。', 'expected': '白茶', 'readback': '白茶'},
    {'id': 'original_choice_only', 'question': '请查我最初对乌龙茶的偏好记录，仅回答当时偏好的饮品名，不要把后来的改动当作最初约定。只写名称。', 'expected': '乌龙茶', 'readback': '乌龙茶'},
    {'id': 'night_adjustment', 'question': '乌龙茶这个旧喜好有没有夜间适用的后续调整？查记录后只写目前晚上优先的饮品，依据不足写未确认。', 'expected': '温水', 'readback': '温水'},
    {'id': 'unrecorded_brand', 'question': '回查我对乌龙茶的偏好，我明确指定的茶叶品牌是什么？只写品牌；没有明确记录就只写未确认。', 'expected': '未确认'},
]
STALE_VIEW_CASES = [
    {'id': 'day_view_check', 'question': '我白天想照个人偏好选饮品。想起以前提过乌龙茶，请查记录后只给现在优先的饮品名；无法核对则只写未确认。', 'expected': '白茶', 'readback': '白茶'},
    {'id': 'old_view_check', 'question': '先找乌龙茶相关记忆，再告诉我最早保存的饮品选择。这里只要当时的名称，不是当前默认。仅输出名称。', 'expected': '乌龙茶', 'readback': '乌龙茶'},
    {'id': 'night_view_check', 'question': '我打算晚上按自己的口味选一杯，记得以前聊过乌龙茶。请看记录，只回复目前夜间该优先的饮品名；没有依据只回复未确认。', 'expected': '温水', 'readback': '温水'},
    {'id': 'brand_view_check', 'question': '关于乌龙茶的记忆，是否记下了我指定的品牌？只能返回记录中明确的品牌名，否则仅输出未确认。', 'expected': '未确认'},
]


def stale_cases(variant=None):
    # v6 deliberately replays the known v5 inputs under the user-approved
    # retrieval contract. It is a regression, not another fresh holdout.
    return {None: STALE_CASES, 'v4': STALE_FOLLOWUP, 'v5': STALE_VIEW_CASES, 'v6': STALE_VIEW_CASES}[variant]


def projection():
    return {'version': 1, 'sources': SOURCES, 'noise': NOISE,
            'cases': [{k: c[k] for k in ('id', 'question')} for c in CASES]}


def sequence_projection(stages=SEQUENCE, version=2):
    return {'version': version, 'noise': NOISE, 'stages': [
        {'id': s['id'], 'sources': s['sources'],
         'probes': [{k: c[k] for k in ('id', 'question', 'clock') if k in c} for c in s['probes']]} for s in stages]}


def temporal_projection(model, followup=False):
    return dict(sequence_projection(TEMPORAL_FOLLOWUP if followup else TEMPORAL, 8 if followup else 7), noise=[], model=model)


def budget_projection(model):
    return dict(sequence_projection(BUDGET, 9), noise=[], model=model)


def without_clock(frame):
    return dict(frame, messages=[dict(m, content=re.sub(
        r'(## 当前时间\n本机当前时间为 )[^。]*(。解释)', r'\1<CLOCK>\2', m.get('content', ''))
        if m.get('role') == 'system' else m.get('content', '')) for m in frame.get('messages', [])])


def assess_temporal(rows, stages=TEMPORAL):
    reviews = [r for r in rows if r.get('phase') == 'learn']
    readers = [r for r in rows if r.get('arm') == 'current']
    cases = [c for s in stages for c in s['probes']]
    checks = {'completeTrace': len(rows) == 8 and len(reviews) == 2 and len(readers) == 6,
              'budget': 0 < sum(r.get('requestCount', 0) for r in rows) <= 20,
              'sameModel': bool(rows) and len({r.get('model') for r in rows}) == 1 and 'luna' in rows[0].get('model', '')}
    source_ids = {}
    for stage in stages:
        row = next((r for r in reviews if r.get('id') == stage['id']), {})
        frames = row.get('providerInputs', [])
        checks[f'{stage["id"]}/review'] = (row.get('status') == 'completed' and row.get('requestCount') == 1 and
            row.get('onlyNewSources') is True and row.get('cursorAdvanced') is True and row.get('inputTokens', 0) > 0 and
            len(frames) == len(row.get('providerOutputs', [])) == 1 and all(source in frames[0].get('messages', [{}])[0].get('content', '') for source in stage['sources']))
        required = stage['evidence_terms']
        source_ids[stage['id']] = [p.get('id') for p in row.get('storedPreferences', []) if p.get('id') and p.get('explicit') and p.get('ready') and
            p.get('content') and p['content'] in stage['sources'][0] and all(word in p['content'] for word in required)]
        checks[f'{stage["id"]}/learnedOriginal'] = bool(source_ids[stage['id']])
        if len(stage['sources']) > 1:
            proposals = row.get('newMemories')
            checks[f'{stage["id"]}/temporaryExcluded'] = isinstance(proposals, list) and all('地铁' not in p.get('content', '') for p in proposals)
            checks[f'{stage["id"]}/temporaryNotRetained'] = not any(p.get('content') and p['content'] in stage['sources'][1]
                for p in row.get('storedPreferences', []))
    exact = 0
    for case in cases:
        row = next((r for r in readers if r.get('id') == case['id']), {})
        frames = row.get('providerInputs', [])
        tools = '\n'.join(m.get('content', '') for f in frames for m in f.get('messages', []) if m.get('role') == 'tool')
        accepted = case.get('accepted', [case['expected']])
        text = (row.get('text') or '').strip()
        exact += text in accepted
        checks[f'{case["id"]}/contract'] = reader_contract(row, case)
        checks[f'{case["id"]}/clock'] = (row.get('clock') == case['clock'] and bool(frames) and any(
            case['clock'] in m.get('content', '') for m in frames[0].get('messages', []) if m.get('role') == 'system'))
        checks[f'{case["id"]}/projection'] = (len(row.get('visible', [])) == 12 and row.get('omitted', 0) > 0 and
            row.get('archiveRecords') == 12 + row.get('omitted', 0) and not any(
                word in value for value in row.get('visible', []) for word in ('通勤', '汇报', '地铁', '自行车', '公交', '打车')))
        checks[f'{case["id"]}/answer'] = text.rstrip('。.') in accepted
        if case.get('source_stage'):
            checks[f'{case["id"]}/sourceReadback'] = any(line.startswith(f'[habit:{source_id}]') for line in tools.splitlines()
                for source_id in source_ids[case['source_stage']])
        if case.get('lookup_required'):
            checks[f'{case["id"]}/lookup'] = any(s.get('tool_name') == 'recall_memories' and s.get('success') is True for s in row.get('steps', []))
    paired = [next((r.get('providerInputs', []) for r in readers if r.get('id') == key), []) for key in (cases[0]['id'], cases[1]['id'])]
    checks['clockOnlyPair'] = bool(paired[0] and paired[1]) and without_clock(paired[0][0]) == without_clock(paired[1][0])
    return {'status': 'passed' if all(checks.values()) else 'needs_work', 'checks': checks,
            'scores': {'current': sum(checks[f'{c["id"]}/answer'] for c in cases), 'exactOutput': exact},
            'usage': {k: sum(r.get(k, 0) for r in readers) for k in ('requestCount', 'inputTokens', 'outputTokens')},
            'learningUsage': {k: sum(r.get(k, 0) for r in reviews) for k in ('requestCount', 'inputTokens', 'outputTokens')}}


def assess_budget(rows):
    reviews = [r for r in rows if r.get('phase') == 'learn']
    readers = [r for r in rows if r.get('arm') in ('low', 'medium')]
    cases = [c for s in BUDGET for c in s['probes']]
    checks = {'completeTrace': len(rows) == 14 and len(reviews) == 2 and len(readers) == 12 and
              len({(r.get('arm'), r.get('id')) for r in readers}) == 12,
              'sameModel': bool(rows) and len({r.get('model') for r in rows}) == 1 and 'luna' in rows[0].get('model', ''),
              'requestBudget': 0 < sum(r.get('requestCount', 0) for r in rows) <= 38,
              'learningAdmitted': len(reviews) == 2 and all(r.get('fixtureAdmitted') is True for r in reviews),
              'usageComplete': bool(rows) and all(r.get('reportedUsageCount') == r.get('requestCount') and r.get('requestCount', 0) > 0 for r in rows),
              'timingComplete': bool(rows) and all(r.get('latencyMs', 0) > 0 for r in rows),
              'fixedLearningEffort': len(reviews) == 2 and all(r.get('providerInputs') and all(
                  f.get('resolvedThinking') == dict(protocol='openai_responses', effort='low', budgetTokens=None) for f in r['providerInputs']) for r in reviews)}
    outcomes, scores, usage = {}, {}, {}
    for arm in ('low', 'medium'):
        arm_rows = [r for r in readers if r.get('arm') == arm]
        outcomes[arm] = assess_temporal(reviews + [dict(r, arm='current') for r in arm_rows], BUDGET)
        flags = outcomes[arm]['checks']
        checks[f'{arm}/sourceLearning'] = all(flags.get(f'{stage["id"]}/{key}') is True
            for stage in BUDGET for key in ('review', 'learnedOriginal'))
        checks[f'{arm}/negativeLearning'] = flags.get('future/temporaryExcluded') is True and flags.get('future/temporaryNotRetained') is True
        checks[f'{arm}/clockOnlyPair'] = flags.get('clockOnlyPair') is True
        checks[f'{arm}/readerContracts'] = all(flags.get(f'{case["id"]}/{key}') is True for case in cases for key in ('contract','clock','projection'))
        scores[arm] = {'answerMatches': outcomes[arm]['scores']['current'], 'strictSuccess': sum(all(
            value is True for key, value in flags.items() if key.startswith(c['id']+'/')) for c in cases)}
        usage[arm] = {k: sum(r.get(k, 0) for r in arm_rows) for k in ('requestCount', 'inputTokens', 'outputTokens', 'latencyMs')}
        usage[arm]['totalTokens'] = usage[arm]['inputTokens'] + usage[arm]['outputTokens']
        checks[f'{arm}/actualEffort'] = len(arm_rows) == 6 and all(r.get('requestedEffort') == arm and r.get('providerInputs') and all(
            f.get('resolvedThinking') == dict(protocol='openai_responses', effort=arm, budgetTokens=None) for f in r['providerInputs']) for r in arm_rows)
    checks['balancedOrder'] = [r.get('arm') for r in readers] == [arm for i in range(6) for arm in (('low','medium') if i % 2 == 0 else ('medium','low'))]
    sequence = []
    index = 0
    for stage in BUDGET:
        sequence.append(('learn', stage['id']))
        for case in stage['probes']:
            sequence.extend((arm,case['id']) for arm in (('low','medium') if index % 2 == 0 else ('medium','low')))
            index += 1
    checks['prefixOrder'] = [(r.get('phase') or r.get('arm'),r.get('id')) for r in rows] == sequence
    prefix_hashes = {r.get('id'):r.get('prefixSha256') for r in reviews}
    checks['differentPrefixes'] = len(prefix_hashes) == 2 and len(set(prefix_hashes.values())) == 2
    for case in cases:
        pair = [next((r for r in readers if r.get('id') == case['id'] and r.get('arm') == arm), {}) for arm in ('low', 'medium')]
        frames = [(r.get('providerInputs') or [{}])[0] for r in pair]
        stage = next(s for s in BUDGET if case in s['probes'])
        checks[f'{case["id"]}/pairedInput'] = bool(frames[0] and frames[1]) and all(
            isinstance(r.get('archiveSha256'),str) and re.fullmatch('[0-9a-f]{64}',r['archiveSha256']) and r.get('archiveUnchanged') is True and
            r['archiveSha256'] == prefix_hashes.get(stage['id']) for r in pair) and (
            {k:v for k,v in frames[0].items() if k != 'resolvedThinking'} == {k:v for k,v in frames[1].items() if k != 'resolvedThinking'})
    valid = all(checks.values())
    medium_ok = valid and scores['medium']['strictSuccess'] == 6
    cost_ok = medium_ok and scores['medium']['strictSuccess'] > scores['low']['strictSuccess'] and usage['low']['totalTokens'] > 0 and (
        usage['medium']['totalTokens'] <= 1.5 * usage['low']['totalTokens'] and usage['medium']['latencyMs'] <= 2 * usage['low']['latencyMs'])
    return {'status': 'passed' if medium_ok else 'needs_work', 'acceptanceScope':'medium_on_v9_fixture',
            'comparisonValid':valid, 'learningAdmissionFailure':any(r.get('fixtureAdmitted') is False for r in reviews),
            'learningIncomplete':len(reviews)!=2, 'checks':checks,
            'outcomes':outcomes, 'scores':scores, 'usage':usage, 'costCandidate':cost_ok,
            'learningUsage':{k:sum(r.get(k,0) for r in reviews) for k in ('requestCount','inputTokens','outputTokens','latencyMs')},
            'configurationDecision':'retain_owner_settings_no_automatic_escalation'}


def local_clock(clock):
    instant = datetime.fromisoformat(clock)
    if instant.utcoffset() is None:
        raise ValueError('Clock view requires an offset-aware instant')
    offset = instant.strftime('%z')
    return (f'{instant.year:04d}年{instant.month:02d}月{instant.day:02d}日'
            f'{instant.hour:02d}时{instant.minute:02d}分{instant.second:02d}秒'
            f'（UTC{offset[:3]}:{offset[3:]}）')


def normalized_clock_frame(frame, rendered):
    marker = f'## 当前时间\n本机当前时间为 {rendered}。解释'
    return dict(frame, messages=[dict(m, content=m.get('content', '').replace(
        marker, '## 当前时间\n本机当前时间为 <CLOCK>。解释')) if m.get('role') == 'system' else m
        for m in frame.get('messages', [])])


def assess_clock_view(rows, model, prefix_hash, future_ids):
    cases = BUDGET[0]['probes'][:2]
    checks = {'completeTrace': len(rows) == 4 and {(r.get('arm'), r.get('id')) for r in rows} ==
              {(arm, c['id']) for arm in ('iso', 'local') for c in cases},
              'noLearning': not any(r.get('phase') == 'learn' for r in rows),
              'sourceBinding': bool(model and future_ids) and bool(re.fullmatch('[0-9a-f]{64}', prefix_hash)),
              'sameModel': bool(rows) and all(r.get('model') == model for r in rows),
              'requestBudget': 0 < sum(r.get('requestCount', 0) for r in rows) <= 12,
              'usageComplete': bool(rows) and all(r.get('requestCount', 0) > 0 and r.get('reportedUsageCount') == r.get('requestCount') for r in rows),
              'timingComplete': bool(rows) and all(r.get('latencyMs', 0) > 0 for r in rows),
              'actualMedium': bool(rows) and all(r.get('requestedEffort') == 'medium' and r.get('providerInputs') and all(
                  f.get('resolvedThinking') == dict(protocol='openai_responses', effort='medium', budgetTokens=None)
                  for f in r['providerInputs']) for r in rows),
              'balancedOrder': [(r.get('arm'), r.get('id')) for r in rows] ==
                  [('iso', cases[0]['id']), ('local', cases[0]['id']), ('local', cases[1]['id']), ('iso', cases[1]['id'])]}
    outcomes, scores, usage, normalized = {}, {}, {}, {}
    for arm in ('iso', 'local'):
        flags = {}
        for case in cases:
            row = next((r for r in rows if r.get('arm') == arm and r.get('id') == case['id']), {})
            rendered = case['clock'] if arm == 'iso' else local_clock(case['clock'])
            frames = row.get('providerInputs', [])
            key = case['id']+'/'
            flags[key+'contract'] = reader_contract(dict(row, arm='current'), case)
            flags[key+'clock'] = row.get('clock') == rendered and row.get('sourceClock') == case['clock'] and bool(frames) and all(
                sum(m.get('content', '').count(f'## 当前时间\n本机当前时间为 {rendered}。解释')
                    for m in f.get('messages', []) if m.get('role') == 'system') == 1 for f in frames)
            flags[key+'prefix'] = row.get('archiveSha256') == prefix_hash and row.get('archiveUnchanged') is True
            flags[key+'projection'] = len(row.get('visible', [])) == 12 and row.get('omitted', 0) > 0 and (
                row.get('archiveRecords') == 12 + row.get('omitted', 0)) and not any(
                    word in value for value in row.get('visible', []) for word in ('通勤', '汇报', '地铁', '自行车', '公交', '步行', '打车'))
            tools = '\n'.join(m.get('content', '') for f in frames for m in f.get('messages', []) if m.get('role') == 'tool')
            flags[key+'sourceReadback'] = any(line.startswith(f'[habit:{source_id}]') for line in tools.splitlines() for source_id in future_ids)
            flags[key+'answer'] = (row.get('text') or '').strip().rstrip('。.') in case.get('accepted', [case['expected']])
            for field in ('contract', 'clock', 'prefix', 'projection'):
                checks[f'{arm}/{key}{field}'] = flags[key+field]
            normalized[(arm, case['id'])] = normalized_clock_frame(frames[0], rendered) if frames else None
        outcomes[arm] = flags
        scores[arm] = dict(answerMatches=sum(flags[c['id']+'/answer'] for c in cases),
            strictSuccess=sum(all(v for k, v in flags.items() if k.startswith(c['id']+'/')) for c in cases))
        arm_rows = [r for r in rows if r.get('arm') == arm]
        usage[arm] = {k:sum(r.get(k,0) for r in arm_rows) for k in ('requestCount','inputTokens','outputTokens','latencyMs')}
        usage[arm]['totalTokens'] = usage[arm]['inputTokens'] + usage[arm]['outputTokens']
    for case in cases:
        pair = [normalized[(arm, case['id'])] for arm in ('iso', 'local')]
        checks[case['id']+'/formatOnlyPair'] = bool(pair[0] and pair[1]) and pair[0] == pair[1]
    for arm in ('iso', 'local'):
        pair = [normalized[(arm, c['id'])] for c in cases]
        checks[arm+'/timeOnlyPair'] = bool(pair[0] and pair[1]) and pair[0] == pair[1]
    valid = all(checks.values())
    target = valid and scores['local']['strictSuccess'] == 2
    return {'status':'passed' if target else 'needs_work', 'acceptanceScope':'local_clock_view_on_v10_prefix',
            'comparisonValid':valid, 'checks':checks, 'outcomes':outcomes, 'scores':scores, 'usage':usage,
            'learningUsage':{k:sum(r.get(k,0) for r in rows if r.get('phase') == 'learn')
                             for k in ('requestCount','inputTokens','outputTokens','latencyMs')},
            'candidateQualifiesForValidation':target and scores['local']['strictSuccess'] > scores['iso']['strictSuccess'],
            'configurationDecision':'retain_owner_settings_pending_independent_validation'}


def conversation_source_readback(row, proof):
    for step in row.get('steps', []):
        if step.get('tool_name') not in ('recall_conversation', 'read_conversation') or step.get('success') is not True:
            continue
        call_id = step.get('call_id')
        output = step.get('output')
        if not isinstance(output, str) or not call_id or not any(m.get('role') == 'tool' and m.get('toolCallId') == call_id and
            m.get('content') == 'Result: '+output for frame in row.get('providerInputs', []) for m in frame.get('messages', [])):
            continue
        try:
            value = json.loads(output)
        except (ValueError, TypeError):
            continue
        if not isinstance(value, dict) or value.get('historical_data') is not True:
            continue
        records = value.get('sources', []) if isinstance(value.get('sources'), list) else []
        records = records + ([value['source']] if isinstance(value.get('source'), dict) else [])
        if any(isinstance(r, dict) and r.get('role') == 'user' and r.get('message_id') in proof.get('messageIds', [])
               and isinstance(r.get('revision'), str) and re.fullmatch('[0-9a-f]{64}', r['revision'])
               and isinstance(r.get('text'), str) and proof.get('quote')
               and proof['quote'] in r.get('text', '') for r in records):
            return True
    return False


def stability_answer(row, case):
    text = (row.get('text') or '').strip()
    if not case.get('structured'):
        return text.rstrip('。.'), True
    fenced = re.fullmatch(r'```(?:json)?\s*\n?(.*?)\n?```', text, flags=re.S | re.I)
    value_text = fenced.group(1).strip() if fenced else text
    try:
        value = json.loads(value_text) if len(value_text) <= 220 else None
    except (ValueError, TypeError):
        value = None
    key = case.get('choice_key', '方式')
    valid = (isinstance(value, dict) and set(value) == {key, '说明'} and
             isinstance(value.get(key), str) and 1 <= len(value[key].strip()) <= 24 and
             isinstance(value.get('说明'), str) and 1 <= len(value['说明'].strip()) <= 80)
    choice = value[key] if isinstance(value, dict) and isinstance(value.get(key), str) else text
    return choice.strip().rstrip('。.'), valid


def stability_proof(prefixes, case):
    prefix = prefixes.get(case.get('source_prefix'), {})
    return prefix.get('caseProofs', {}).get(case['id'], prefix)


def initial_preference_evidence(row, proof):
    quote = proof.get('quote')
    return bool(quote and quote in row.get('visible',[]) and row.get('providerInputs') and any(
        m.get('role')=='system' and quote in m.get('content','') for m in row['providerInputs'][0].get('messages',[])))


def without_preference_frame(frame):
    if not frame: return frame
    normalized = deepcopy(frame)
    def mask_values(section):
        # Only stored explicit values vary. Keep instructions, record labels,
        # scope, confidence, counts and all other content in the comparison.
        return re.sub(r'^(- user-stated preference: )"(?:[^"\\\r\n]|\\.)*"(?= \([^\r\n]* scope, confidence \d+%\)$)',
                      r'\1"<stored value>"', section.group(0), flags=re.M)
    for message in normalized.get('messages',[]):
        if message.get('role')=='system':
            message['content']=re.sub(r'## Learned interaction defaults\n.*?(?=\n\n## |\Z)',
                                     mask_values,message.get('content',''),flags=re.S)
    return normalized


def narrow_preference_frame(frame, visible, omitted):
    """Remove only the ten known filler records and adjust the omission count."""
    normalized = deepcopy(frame)
    systems = [m for m in (normalized or {}).get('messages',[]) if m.get('role')=='system']
    if len(systems) != 1 or len(visible) != 12:
        return None
    text = systems[0].get('content','')
    header = f'Working view: 12 shown; {omitted} additional eligible records omitted.'
    if text.count(header) != 1:
        return None
    for value in visible[2:]:
        pattern = r'^- user-stated preference: '+re.escape(json.dumps(value,ensure_ascii=False))+r' \([^\r\n]* scope, confidence \d+%\)\n?'
        text, count = re.subn(pattern,'',text,flags=re.M)
        if count != 1:
            return None
    systems[0]['content'] = text.replace(header,f'Working view: 2 shown; {omitted+10} additional eligible records omitted.')
    return normalized


def assess_stability(rows, model, prefixes, sources=False, scenario='original'):
    cases = stability_cases(sources, scenario)
    window_pair = scenario == 'view-budget'
    guard_pair = scenario == 'lookup-guards'
    task_view = scenario in ('counter-view','view-budget','lookup-guards')
    nominees = WINDOW_SOURCES if window_pair else COUNTER_SOURCES
    count = len(cases)
    tool_names = ('recall_memories', 'recall_conversation', 'read_conversation') if sources else ('recall_memories',)
    checks = dict(completeTrace=len(rows) == 2*count and len({(r.get('trial'), r.get('id')) for r in rows}) == 2*count,
        noLearning=not any(r.get('phase') == 'learn' for r in rows),
        sourceBindings=set(prefixes) == {0, 1} and all(p.get('sourceIds') and
            re.fullmatch('[0-9a-f]{64}', p.get('sha256', '')) for p in prefixes.values()),
        sameModel=bool(model and rows) and all(r.get('model') == model for r in rows),
        requestBudget=0 < sum(r.get('requestCount', 0) for r in rows) <= (8 if sources else 6)*count,
        usageComplete=bool(rows) and all(r.get('requestCount', 0) > 0 and r.get('reportedUsageCount') == r.get('requestCount') for r in rows),
        timingComplete=bool(rows) and all(r.get('latencyMs', 0) > 0 for r in rows),
        actualMedium=bool(rows) and all(r.get('requestedEffort') == 'medium' and r.get('providerInputs') and all(
            f.get('resolvedThinking') == dict(protocol='openai_responses', effort='medium', budgetTokens=None)
            for f in r['providerInputs']) for r in rows),
        balancedOrder=[(r.get('trial'), r.get('id')) for r in rows] ==
            [(trial, c['id']) for trial in (1, 2) for c in (cases if trial == 1 else cases[::-1])])
    if sources:
        checks['originalBindings'] = all(stability_proof(prefixes,c).get('messageIds') and stability_proof(prefixes,c).get('quote') for c in cases if 'source_prefix' in c)
    outcomes, scores, first_frames, strict, canonical = {}, {}, {}, {}, {}
    aliases = {word:case['expected'] for case in (GUARDS if guard_pair else WINDOW_CASES if window_pair else COUNTER if scenario in ('counter','counter-view') else DAILY if scenario == 'daily' else CONDITIONS if scenario == 'conditions' else STABILITY)
               for word in case.get('accepted', [case['expected']])}
    for trial in (1, 2):
        flags = {}
        for index, case in enumerate(cases):
            row = next((r for r in rows if r.get('id') == case['id'] and r.get('trial') == trial), {})
            frames = row.get('providerInputs', [])
            key = case['id'] + '/'
            flags[key+'contract'] = row.get('arm') == 'stable' and reader_contract(dict(row, arm='current'), case, tool_names, 4 if sources else 3)
            if sources:
                flags[key+'contract'] = flags[key+'contract'] and row.get('sourceReaders') is True
            if task_view:
                flags[key+'contract'] = flags[key+'contract'] and row.get('taskView') is True
            slots = 2 if window_pair and (index+trial)%2 == 0 else 12
            if window_pair:
                flags[key+'contract'] = flags[key+'contract'] and row.get('viewSlots') == slots
            if guard_pair:
                flags[key+'contract'] = flags[key+'contract'] and row.get('lookupAware') is ((index+trial)%2==0)
            flags[key+'clock'] = row.get('clock') == case['clock'] and bool(frames) and all(
                sum(m.get('content', '').count(f'## 当前时间\n本机当前时间为 {case["clock"]}。解释')
                    for m in f.get('messages', []) if m.get('role') == 'system') == 1 for f in frames)
            flags[key+'prefix'] = row.get('prefix') == case['prefix'] and row.get('archiveUnchanged') is True and (
                row.get('archiveSha256') == prefixes.get(case['prefix'], {}).get('sha256'))
            flags[key+'projection'] = len(row.get('visible', [])) == slots and row.get('omitted', 0) > 0 and (
                row.get('archiveRecords') == slots + row.get('omitted', 0)) and (task_view or not any(
                    word in value for value in row.get('visible', []) for word in ('通勤', '汇报', '地铁', '自行车', '公交', '步行', '打车'))
                )
            for field in ('contract', 'clock', 'prefix', 'projection'):
                checks[f'{trial}/{key}{field}'] = flags[key+field]
            tools = '\n'.join(m.get('content', '') for f in frames for m in f.get('messages', []) if m.get('role') == 'tool')
            if 'source_prefix' in case:
                proof = stability_proof(prefixes, case)
                flags[key+'sourceReadback'] = any(line.startswith(f'[habit:{source_id}]') for line in tools.splitlines()
                    for source_id in proof.get('sourceIds', [])) or (sources and conversation_source_readback(row, proof))
                if task_view:
                    flags[key+'sourceReadback'] |= initial_preference_evidence(row,proof)
            if case.get('lookup_required'):
                flags[key+'lookup'] = any(s.get('tool_name') == 'recall_memories' and s.get('success') is True for s in row.get('steps', []))
                if guard_pair:
                    flags[key+'lookup'] = any(s.get('tool_name') in ('recall_memories','recall_conversation') and s.get('success') is True and s.get('call_id') and isinstance(s.get('output'),str) and any(
                        m.get('role')=='tool' and m.get('toolCallId')==s['call_id'] and m.get('content')=='Result: '+s['output']
                        for f in frames for m in f.get('messages',[])) for s in row.get('steps',[]))
            text, shaped = stability_answer(row, case)
            if case.get('structured'):
                flags[key+'format'] = shaped
            flags[key+'answer'] = text in case.get('accepted', [case['expected']])
            canonical[(trial, case['id'])] = aliases.get(text, text)
            strict[(trial, case['id'])] = all(v for k, v in flags.items() if k.startswith(key))
            first_frames[(trial, case['id'])] = frames[0] if frames else None
        outcomes[str(trial)] = flags
        scores[str(trial)] = dict(answerMatches=sum(flags[c['id']+'/answer'] for c in cases),
            strictSuccess=sum(strict[(trial, c['id'])] for c in cases))
    for case in cases:
        pair = [first_frames[(trial, case['id'])] for trial in (1, 2)]
        if guard_pair:
            current = next((r for r in rows if r.get('id')==case['id'] and r.get('lookupAware') is False),{})
            aware = next((r for r in rows if r.get('id')==case['id'] and r.get('lookupAware') is True),{})
            a,b = ((r.get('providerInputs') or [None])[0] for r in (current,aware))
            try:
                same = bool(a and b) and lookup_frames(a)[1] == b
            except (ValueError,KeyError,TypeError):
                same = False
            checks[case['id']+'/lookupOnlyInput'] = same and current.get('visible')==aware.get('visible') and current.get('omitted')==aware.get('omitted')
        elif window_pair:
            pair_rows = [r for r in rows if r.get('id')==case['id']]
            full = next((r for r in pair_rows if r.get('viewSlots')==12),{})
            lean = next((r for r in pair_rows if r.get('viewSlots')==2),{})
            full_frame = (full.get('providerInputs') or [None])[0]
            lean_frame = (lean.get('providerInputs') or [None])[0]
            checks[case['id']+'/windowOnlyInput'] = bool(full_frame and lean_frame) and (
                full.get('visible',[])[:2] == lean.get('visible') and
                full.get('omitted',0)+10 == lean.get('omitted') and
                narrow_preference_frame(full_frame,full.get('visible',[]),full.get('omitted')) == lean_frame)
        else:
            checks[case['id']+'/repeatInputIdentical'] = bool(pair[0] and pair[1]) and pair[0] == pair[1]
    if scenario in ('counter','counter-view','view-budget'):
        quotes = [quote for source in nominees for quote in source['quotes']]
        if scenario == 'counter':
            checks['counterTargetsAbsent'] = all(not any(quote in json.dumps(first_frames[(trial,c['id'])],ensure_ascii=False) for quote in quotes)
                for trial in (1,2) for c in cases)
        for trial in (1, 2):
            for source in (() if window_pair else nominees):
                a, b = (first_frames[(trial, f'{source["id"]}_{prefix}')] for prefix in (0, 1))
                checks[f'{trial}/{source["id"]}/profileOnlyInput'] = bool(a and b) and (
                    without_preference_frame(a) == without_preference_frame(b) if scenario == 'counter-view' else a == b)
        if window_pair:
            for slots in (12,2):
                for source in nominees:
                    pair = [next((r for r in rows if r.get('id')==f'{source["id"]}_{p}' and r.get('viewSlots')==slots),{}) for p in (0,1)]
                    frames = [(r.get('providerInputs') or [None])[0] for r in pair]
                    checks[f'{slots}/{source["id"]}/profileOnlyInput'] = bool(all(frames)) and without_preference_frame(frames[0]) == without_preference_frame(frames[1])
    if scenario in ('conditions', 'daily'):
        for trial in (1, 2):
            identifiers = ('daily_current', 'daily_rain') if scenario == 'daily' else ('condition_current', 'condition_rain')
            pair = [first_frames[(trial, identifier)] for identifier in identifiers]
            normalized = [dict(frame, messages=[dict(m, content='<weather question>') if i == len(frame['messages'])-1 else m
                for i, m in enumerate(frame['messages'])]) for frame in pair if frame]
            checks[f'{trial}/weatherOnlyInput'] = len(normalized) == 2 and normalized[0] == normalized[1]
    if guard_pair:
        for aware in (False,True):
            pair = [next((r for r in rows if r.get('id')==case['id'] and r.get('lookupAware') is aware),{}) for case in GUARDS[:2]]
            frames = [(r.get('providerInputs') or [None])[0] for r in pair]
            checks[str(aware)+'/clockOnlyBoundary'] = bool(all(frames)) and without_clock(frames[0])==without_clock(frames[1])
    for trial in (() if sources else (1, 2)):
        pair = [first_frames[(trial, case['id'])] for case in STABILITY[:2]]
        checks[f'{trial}/clockOnlyBoundary'] = bool(pair[0] and pair[1]) and without_clock(pair[0]) == without_clock(pair[1])
    stable = dict(cases=count, trials=2,
        allCasesBothPass=sum(all(strict[(trial, c['id'])] for trial in (1, 2)) for c in cases),
        minTrialStrict=min(s['strictSuccess'] for s in scores.values()),
        answerInconsistency=sum(canonical[(1, c['id'])] != canonical[(2, c['id'])] for c in cases),
        strictInconsistency=sum(strict[(1, c['id'])] != strict[(2, c['id'])] for c in cases))
    # Keep historical score fields unchanged. Interrupted matrices separately
    # report actual observations: a missing/failed reader is not an answer flip.
    paired = [c for c in cases if checks[c['id']+('/lookupOnlyInput' if guard_pair else '/windowOnlyInput' if window_pair else '/repeatInputIdentical')] and
              all(outcomes[str(t)][c['id']+'/contract'] for t in (1, 2))]
    observed = dict(expected=2*count, received=len(rows),
        completed=sum(flags[c['id']+'/contract'] for flags in outcomes.values() for c in cases),
        answerMatches=sum(flags[c['id']+'/contract'] and flags[c['id']+'/answer'] for flags in outcomes.values() for c in cases),
        strictSuccess=sum(strict.values()), pairedCases=len(paired),
        pairedAnswerChanges=sum(canonical[(1, c['id'])] != canonical[(2, c['id'])] for c in paired),
        pairedStrictChanges=sum(strict[(1, c['id'])] != strict[(2, c['id'])] for c in paired))
    valid = all(checks.values())
    report = dict(status='passed' if valid and stable['allCasesBothPass'] == count else 'needs_work',
        acceptanceScope='completed_lookup_guards_on_v20' if guard_pair else 'paired_task_windows_on_v18' if window_pair else 'task_matched_profiles_on_v17' if scenario == 'counter-view' else 'counterfactual_profiles_on_v16' if scenario == 'counter' else 'natural_task_cards_on_v15' if scenario == 'daily' else 'explicit_conditions_on_v13' if scenario == 'conditions' else 'existing_conversation_readers_on_v12' if sources else 'two_cold_trials_on_v11_wording', comparisonValid=valid, checks=checks, outcomes=outcomes,
        scores=scores, stability=stable, observed=observed,
        initialSourceEvidence=sum(initial_preference_evidence(r,stability_proof(prefixes,c)) for r in rows for c in cases if task_view and r.get('id')==c['id']),
        sourceReadbacks=sum(conversation_source_readback(r, stability_proof(prefixes, c))
            for r in rows for c in cases if sources and r.get('id') == c['id'] and 'source_prefix' in c),
        usage={k:sum(r.get(k, 0) for r in rows) for k in ('requestCount', 'inputTokens', 'outputTokens', 'latencyMs')},
        learningUsage={k:sum(r.get(k, 0) for r in rows if r.get('phase') == 'learn')
                       for k in ('requestCount', 'inputTokens', 'outputTokens')},
        configurationDecision='frozen_core_no_automatic_escalation')
    if window_pair or guard_pair:
        if guard_pair:
            report['pairedContracts'] = dict(cases=count,bothContractsPassed=stable['allCasesBothPass'])
        else:
            report['pairedViews'] = dict(cases=count, bothViewsPassed=stable['allCasesBothPass'])
        del report['stability']  # Two treatments, not repeated identical inputs.
        group_key = 'scoresByContract' if guard_pair else 'scoresByView'
        report[group_key] = {}
        for value, label in ((False,'current_contract'),(True,'lookup_aware')) if guard_pair else ((12,'12'),(2,'2')):
            group = [r for r in rows if r.get('lookupAware' if guard_pair else 'viewSlots')==value]
            report[group_key][label] = dict(readers=len(group),
                completedReaders=sum(r.get('status')=='completed' for r in group),
                meteredRequests=sum(r.get('reportedUsageCount',0) for r in group),
                unmeteredRequests=sum(max(0,r.get('requestCount',0)-r.get('reportedUsageCount',0)) for r in group),
                usageComplete=len(group)==count and all(r.get('requestCount',0)>0 and r.get('requestCount')==r.get('reportedUsageCount') for r in group),
                strictSuccess=sum(strict.get((r.get('trial'),r.get('id')),False) for r in group),
                usage={k:sum(r.get(k,0) for r in group) for k in ('requestCount','inputTokens','outputTokens','latencyMs')})
        if guard_pair:
            bound = all(checks[k] for k in ('sourceBindings','originalBindings','sameModel','actualMedium','noLearning'))
            comparable = [c['id'] for c in paired if bound and all(outcomes[str(t)][c['id']+'/'+field] for t in (1,2) for field in ('clock','prefix','projection'))]
            report['matchedPairUsage'] = dict(cases=comparable,scope='completed, metered, input-matched observations only; excludes unmatched attempts and unknown cost',
                arms={label:{k:sum(r.get(k,0) for r in rows if r.get('id') in comparable and r.get('lookupAware') is aware)
                    for k in ('requestCount','inputTokens','outputTokens','latencyMs')} for aware,label in ((False,'current_contract'),(True,'lookup_aware'))})
        baseline, lean = (report[group_key][label]['usage']['inputTokens'] for label in (('current_contract','lookup_aware') if guard_pair else ('12','2')))
        report['inputReductionRatio'] = 1-lean/baseline if valid and baseline else None
    return report


def saved_prefix_source(path, stages=(0,), sources=False):
    source = Path(path).resolve()
    if source.parent != (ROOT / 'logs').resolve() or not source.name.startswith('minimal-architecture-'):
        raise ValueError('Clock view requires an owned recording under logs')
    files = [source / 'acceptance.json'] + [source / f'prefix-{stage}.db' for stage in stages]
    if any(p.resolve().parent != source for p in files):
        raise ValueError('Clock-view source files must remain inside their recording')
    hashes = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in files}
    report = json.loads(files[0].read_text(encoding='utf-8'))
    if (report.get('experiment') != 'budget_pair_v9' or report.get('exitCode') != 0 or
        report.get('comparisonValid') is not True or report.get('sourceUnchanged') is not True or
        assess_budget(report.get('results', []))['comparisonValid'] is not True):
        raise ValueError('Clock view requires a completed method-valid v9, not necessarily business passed')
    prefixes = {}
    for stage in stages:
        learned = next(r for r in report['results'] if r.get('phase') == 'learn' and r.get('id') == BUDGET[stage]['id'])
        filename = f'prefix-{stage}.db'
        snapshot = learned.get('prefixSnapshot', {})
        if snapshot.get('file') != filename or snapshot.get('sha256') != hashes[filename]:
            raise ValueError('Prefix snapshot checksum does not match its learning receipt')
        prefix_hash = learned.get('prefixSha256', '')
        if not re.fullmatch('[0-9a-f]{64}', prefix_hash):
            raise ValueError('Missing prefix logical identity')
        candidates = [p for p in learned.get('storedPreferences', []) if p.get('explicit') and p.get('ready') and
                      p.get('content') == BUDGET[stage]['sources'][0] and p.get('id')]
        message_ids = set()
        with contextlib.closing(sqlite3.connect(f'file:{(source/filename).as_posix()}?mode=ro', uri=True)) as conn:
            for p in candidates:
                stored = conn.execute('SELECT constraint_value,source,status FROM adaptive_constraints WHERE id=?', (p['id'],)).fetchone()
                if stored != (p['content'], 'explicit_user_preference', 'active'):
                    raise ValueError('Source identity or original quote does not match its snapshot')
                if sources:
                    message_ids.update(row[0] for row in conn.execute(
                        "SELECT m.id FROM adaptive_constraint_evidence e JOIN messages m ON e.source='explicit_user:'||m.id AND e.session_id=m.session_id "
                        "WHERE e.constraint_key=(SELECT constraint_key FROM adaptive_constraints WHERE id=?) AND m.role='user' AND m.is_provisional=0 AND m.content=?",
                        (p['id'], p['content'])))
        if not candidates:
            raise ValueError('Required source missing from prefix receipt')
        prefixes[stage] = dict(sha256=prefix_hash, sourceIds=[p['id'] for p in candidates], file=filename)
        if sources:
            if not message_ids:
                raise ValueError('Original user-message evidence is unavailable')
            prefixes[stage].update(messageIds=sorted(message_ids), quote=BUDGET[stage]['sources'][0])
    if any(hashlib.sha256((source / name).read_bytes()).hexdigest() != digest for name,digest in hashes.items()):
        raise ValueError('Source changed while being inspected')
    return source, hashes, report['results'][0]['model'], prefixes


def clock_view_source(path):
    source, hashes, model, prefixes = saved_prefix_source(path)
    return source, hashes, model, prefixes[0]['sha256'], prefixes[0]['sourceIds']


def run_clock_view(live, source_path):
    source, hashes, model, prefix_hash, future_ids = clock_view_source(source_path)
    cases = BUDGET[0]['probes'][:2]
    payload = dict(version=10, live=True, model=model, cases=[{k:c[k] for k in ('id','question','clock')} for c in cases])
    plan = dict(learningCalls=0, readerDecisions=4, maxProviderCalls=12, softInputStop=35000,
                scope='same saved future prefix; representation screen, not a long-term or production acceptance')
    if not live:
        return dict(status='offline_only', plan=plan, input=payload, sourceSha256=hashes)
    owned = Path(tempfile.mkdtemp(prefix='minimal-architecture-', dir=ROOT / 'logs')).resolve()
    report = dict(status='blocked', experiment='clock_view_v10', checkedAt=datetime.now(timezone.utc).isoformat(),
        source=str(source), sourceSha256=hashes, prefixSha256=prefix_hash, plan=plan,
        fixtureSha256=hashlib.sha256(json.dumps(dict(input=payload,cases=cases),ensure_ascii=False,sort_keys=True).encode()).hexdigest(),
        codeSha256={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in (
            'scripts/prototypes/minimal_architecture.py','src-tauri/src/commands/message_live_tests/minimal_architecture.rs',
            'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs','src-tauri/src/agent/conversation/prompt.rs',
            'src-tauri/src/agent/extensions/memory.rs','src-tauri/src/agent/support/adaptive_constraints.rs','src-tauri/src/fts.rs')})
    target = owned / 'acceptance.json'
    print(f'MINIMAL_ARCHITECTURE_REPORT {target}', flush=True)
    with contextlib.closing(sqlite3.connect(f'file:{(source/"prefix-0.db").as_posix()}?mode=ro',uri=True)) as original, contextlib.closing(sqlite3.connect(owned/'state.db')) as copied:
        original.backup(copied)
    (owned/'input.json').write_text(json.dumps(payload,ensure_ascii=False),encoding='utf-8')
    try:
        result, rows = run_context_worker(owned, WORKER+'_stale', 'MINIMAL_ARCHITECTURE_ROW ', timeout=520)
        report.update(assess_clock_view(rows,model,prefix_hash,future_ids), results=rows, exitCode=result.returncode)
        report['sourceUnchanged'] = all(hashlib.sha256((source/name).read_bytes()).hexdigest()==digest for name,digest in hashes.items())
        report['usageIncomplete'] = result.returncode != 0 or len(rows) != 4 or any(r.get('reportedUsageCount')!=r.get('requestCount') for r in rows)
        report['checks'].update(workerSucceeded=result.returncode==0, sourceUnchanged=report['sourceUnchanged'])
        if result.returncode or not report['sourceUnchanged']:
            report.update(status='blocked',comparisonValid=False)
    finally:
        target.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    return report


def counter_prefixes(owned, sources=None):
    """Verify the worker's prepared references against both owned databases."""
    sources = COUNTER_SOURCES if sources is None else sources
    target = owned/'counter-prefixes.json'
    if target.resolve().parent != owned:
        raise ValueError('Counterfactual references must remain in the owned recording')
    raw = target.read_bytes()
    prefixes = {int(k):v for k,v in json.loads(raw).items()}
    if set(prefixes) != {0,1}:
        raise ValueError('Both counterfactual profiles must be prepared')
    for prefix, metadata in prefixes.items():
        path = owned/f'prefix-{prefix}'/'state.db'
        if path.resolve().parent.parent != owned:
            raise ValueError('Counterfactual database must remain inside its owned recording')
        expected_keys = {f'{s["id"]}_{prefix}' for s in sources}
        if set(metadata.get('caseProofs', {})) != expected_keys or not re.fullmatch('[0-9a-f]{64}',metadata.get('sha256','')):
            raise ValueError('Counterfactual preparation proof is incomplete')
        with contextlib.closing(sqlite3.connect(f'file:{path.as_posix()}?mode=ro',uri=True)) as conn:
            session = conn.execute("SELECT session_id FROM messages WHERE id='seq-0-0'").fetchone()[0]
            for source in sources:
                proof = metadata['caseProofs'][f'{source["id"]}_{prefix}']
                quote = source['quotes'][prefix]
                rows = conn.execute("SELECT a.id,a.constraint_value,m.content FROM adaptive_constraints a JOIN adaptive_constraint_evidence e ON a.constraint_key=e.constraint_key JOIN messages m ON e.source='explicit_user:'||m.id AND e.session_id=m.session_id WHERE m.id=? AND m.session_id=? AND m.role='user' AND m.is_provisional=0 AND a.source='explicit_user_preference' AND a.status='active'",(source['id'],session)).fetchall()
                if (proof.get('messageIds') != [source['id']] or proof.get('quote') != quote or
                    len(rows) != 1 or rows[0][1:] != (quote,quote) or proof.get('sourceIds') != [rows[0][0]]):
                    raise ValueError('Counterfactual quote and actual retained evidence do not match')
        if metadata.get('messageIds') != [s['id'] for s in sources] or metadata.get('sourceIds') != [metadata['caseProofs'][f'{s["id"]}_{prefix}']['sourceIds'][0] for s in sources]:
            raise ValueError('Counterfactual profile references are inconsistent')
    if target.read_bytes() != raw:
        raise ValueError('Counterfactual preparation changed while being read')
    return prefixes


def run_stability(live, source_path, sources=False, scenario='original'):
    cases = stability_cases(sources, scenario)
    nominees = WINDOW_SOURCES if scenario == 'view-budget' else COUNTER_SOURCES if scenario in ('counter','counter-view') else []
    source, hashes, model, prefixes = saved_prefix_source(source_path, (0, 1), sources)
    payload = dict(version=20 if scenario == 'lookup-guards' else 18 if scenario == 'view-budget' else 17 if scenario == 'counter-view' else 16 if scenario == 'counter' else 15 if scenario == 'daily' else 13 if scenario == 'conditions' else 12 if sources else 11, live=True, model=model,
                   cases=[{k:c[k] for k in ('id', 'question', 'clock', 'prefix')} for c in cases])
    if nominees:
        payload['counter_sources'] = nominees
    plan = dict(learningCalls=0, cases=len(cases), trials=2, readerDecisions=2*len(cases), maxProviderCalls=(8 if sources else 6)*len(cases),
                softInputStop=100000, scope='two manually nominated profiles; task-matched view, same 12 slots' if scenario == 'counter-view' else 'two manually nominated profiles, identical natural tasks, no new learning' if scenario == 'counter' else 'natural task cards, history and one-off choice; no business side effects' if scenario == 'daily' else 'explicit weather and historical/current branches' if scenario == 'conditions' else 'known withdrawal/history with existing source readers' if sources else 'fresh wording on two saved knowledge prefixes, not natural multi-day use')
    if scenario == 'counter-view':
        plan['taskViewPolicy'] = 'strongest lexical match plus newest matching candidate within existing top-50 cap; at most 2 retained, 12 total'
    if scenario == 'view-budget':
        plan.update(scope='two new topics; same task selector, 12 versus 2 records, each case once per view',
            viewSlots=[12,2], pairing='first round 12/2/12/2, reverse round 12/2/12/2; each case gets both sizes',
            taskViewPolicy='v17 unchanged; reuse existing truncation only, keep omitted-record guidance and all read tools')
    if scenario == 'lookup-guards':
        plan.update(scope='six boundary cards, both lookup contracts once; real read-only tool execution, not long-term stability',
            taskViewPolicy='v17 selection and 12 slots unchanged',lookupPolicy=LOOKUP_AWARE,
            pairing='alternating contracts by case; reverse second round, each case gets both contracts')
    if not live:
        return dict(status='offline_only', input=payload, plan=plan, sourceSha256=hashes)
    owned = Path(tempfile.mkdtemp(prefix='minimal-architecture-', dir=ROOT / 'logs')).resolve()
    code_files = ('scripts/prototypes/minimal_architecture.py', 'scripts/context_eval.py', 'src-tauri/src/commands/message_live_tests/minimal_architecture.rs',
        'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs', 'src-tauri/src/agent/conversation/prompt.rs',
        'src-tauri/src/agent/extensions/memory.rs', 'src-tauri/src/agent/support/adaptive_constraints.rs', 'src-tauri/src/fts.rs',
        'src-tauri/src/agent/tools/registry.rs')
    if sources:
        code_files += ('src-tauri/src/commands/foreground_context_recall.rs',)
    report = dict(status='preparing', comparisonValid=False, usageIncomplete=True, experiment='completed_lookup_guards_v20' if scenario == 'lookup-guards' else 'paired_task_windows_v18' if scenario == 'view-budget' else 'task_matched_profiles_v17' if scenario == 'counter-view' else 'counterfactual_profiles_v16' if scenario == 'counter' else 'natural_tasks_v15' if scenario == 'daily' else 'explicit_conditions_v13' if scenario == 'conditions' else 'source_readers_v12' if sources else 'stability_v11', checkedAt=datetime.now(timezone.utc).isoformat(),
        source=str(source), sourceSha256=hashes, prefixes=prefixes, plan=plan,
        fixtureSha256=hashlib.sha256(json.dumps(dict(input=payload, cases=cases), ensure_ascii=False, sort_keys=True).encode()).hexdigest(),
        codeSha256={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in code_files})
    target = owned / 'acceptance.json'
    print(f'MINIMAL_ARCHITECTURE_REPORT {target}', flush=True)

    def persist():
        temporary = target.with_suffix('.tmp')
        temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        temporary.replace(target)

    persist()
    try:
        for prefix, info in prefixes.items():
            directory = owned / f'prefix-{prefix}'
            directory.mkdir()
            base_file = prefixes[1]['file'] if nominees else info['file']
            with contextlib.closing(sqlite3.connect(f'file:{(source/base_file).as_posix()}?mode=ro', uri=True)) as original, contextlib.closing(sqlite3.connect(directory/'state.db')) as copied:
                original.backup(copied)
        (owned/'input.json').write_text(json.dumps(payload, ensure_ascii=False), encoding='utf-8')
        report['status'] = 'running'
        persist()
        result, rows = run_context_worker(owned, WORKER+'_stale', 'MINIMAL_ARCHITECTURE_ROW ', timeout=1700)
        if nominees:
            prefixes = counter_prefixes(owned,nominees)
            report['prefixes'] = prefixes
        report.update(assess_stability(rows, model, prefixes, sources, scenario), results=rows, exitCode=result.returncode)
        report['sourceUnchanged'] = all(hashlib.sha256((source/name).read_bytes()).hexdigest() == digest for name, digest in hashes.items())
        report['codeUnchanged'] = all(hashlib.sha256((ROOT/name).read_bytes()).hexdigest() == digest for name,digest in report['codeSha256'].items())
        report['usageIncomplete'] = result.returncode != 0 or len(rows) != 2*len(cases) or any(r.get('reportedUsageCount') != r.get('requestCount') for r in rows)
        report['checks'].update(workerSucceeded=result.returncode == 0, sourceUnchanged=report['sourceUnchanged'],codeUnchanged=report['codeUnchanged'])
        if result.returncode or not report['sourceUnchanged'] or not report['codeUnchanged']:
            report.update(status='blocked', comparisonValid=False)
    except BaseException as error:
        report.update(status='interrupted', comparisonValid=False, interruptionType=type(error).__name__, usageIncomplete=True)
        try:
            persist()
        except Exception as preservation_error:
            error.add_note(f'Could not persist interruption manifest ({type(preservation_error).__name__}).')
        raise
    else:
        persist()
    return report


def order_frames(frame):
    """Only stable-partition the recorded source list; no selection or rewriting."""
    original, changed = deepcopy(frame), deepcopy(frame)
    if [m.get('role') for m in original.get('messages', [])] != ['system', 'user', 'assistant', 'tool']:
        raise ValueError('Order screen needs one recorded search result')
    if [t.get('name') for t in original.get('tools', [])] != ['read_conversation', 'recall_conversation', 'recall_memories']:
        raise ValueError('Only the recorded three read-only tools are allowed')
    content = original['messages'][-1]['content']
    if not content.startswith('Result: '):
        raise ValueError('Expected the exact successful production envelope')
    value = json.loads(content[len('Result: '):])
    if value.get('historical_data') is not True or not 2 <= len(value.get('sources', [])) <= 10 or any(
            type(s.get('matched')) is not bool or not isinstance(s.get('text'), str) for s in value['sources']):
        raise ValueError('Every bounded source must retain its original match marker and text')
    value['sources'].sort(key=lambda s:not s['matched'])
    changed['messages'][-1]['content'] = 'Result: '+json.dumps(value, ensure_ascii=False, separators=(',', ':'), sort_keys=True)
    return original, changed


LOOKUP_CURRENT = 'When current or historical preferences matter and authorized lookup is available, use recall_memories (user-stated preferences are included by default); set include_learned_habits=true only to inspect inferred habits.'
LOOKUP_AWARE = 'A host lookup for this request already placed up to two matching user-stated candidates first; the rest are recent records. Use relevant shown originals when sufficient for this task; use recall_memories for missing details or unresolved later corrections and conditions. Do not repeat lookup solely because the view has omissions. User-stated preferences are included by default; set include_learned_habits=true only to inspect inferred habits.'


def lookup_frames(frame):
    if ([m.get('role') for m in frame.get('messages',[])] != ['system','user'] or
        [t.get('name') for t in frame.get('tools',[])] != ['read_conversation','recall_conversation','recall_memories'] or
        frame.get('resolvedThinking') != dict(protocol='openai_responses',effort='medium',budgetTokens=None) or
        frame['messages'][0].get('content','').count(LOOKUP_CURRENT) != 1):
        raise ValueError('Lookup screen needs the unmodified recorded task-view contract')
    changed = deepcopy(frame)
    changed['messages'][0]['content'] = changed['messages'][0]['content'].replace(LOOKUP_CURRENT,LOOKUP_AWARE)
    return deepcopy(frame),changed


def order_source(path, lookup=False):
    source = Path(path).resolve()
    if source.parent != (ROOT/'logs').resolve() or not source.name.startswith('minimal-architecture-'):
        raise ValueError('Order screen requires an owned synthetic v13 recording')
    target = source/'acceptance.json'
    if target.resolve().parent != source:
        raise ValueError('Recording must stay in its owned directory')
    raw = target.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    report = json.loads(raw)
    if lookup:
        if report.get('experiment') != 'paired_task_windows_v18' or report.get('exitCode') != 0 or report.get('usageIncomplete') is not False:
            raise ValueError('Lookup screen needs a fully metered v18, not a successful-score filter')
        _, hashes, model, _ = saved_prefix_source(report['source'],(0,1),True)
        prefixes = counter_prefixes(source,WINDOW_SOURCES)
        if report.get('sourceUnchanged') is not True or report.get('sourceSha256') != hashes or not assess_stability(report['results'],model,prefixes,True,'view-budget')['comparisonValid']:
            raise ValueError('Recorded task-view contract is invalid')
        row = next(r for r in report['results'] if r['trial']==1 and r['id']==WINDOW_CASES[0]['id'])
        if row['viewSlots'] != 12 or not initial_preference_evidence(row,stability_proof(prefixes,WINDOW_CASES[0])):
            raise ValueError('First recorded reader must contain its actual task-selected declaration')
        frame = row['providerInputs'][0]
        lookup_frames(frame)
        if target.read_bytes() != raw:
            raise ValueError('Source recording changed while loading')
        return source,digest,model,frame
    if report.get('experiment') != 'explicit_conditions_v13' or report.get('exitCode') != 0 or report.get('usageIncomplete') is not False:
        raise ValueError('Order screen requires a completed, fully metered v13')
    _, hashes, model, prefixes = saved_prefix_source(report['source'], (0, 1), True)
    if report.get('sourceUnchanged') is not True or report.get('sourceSha256') != hashes or not assess_stability(report['results'], model, prefixes, True, 'conditions')['comparisonValid']:
        raise ValueError('Recorded v13 contract is invalid')
    row = next(r for r in report['results'] if r['trial'] == 1 and r['id'] == 'condition_history')
    if not all(conversation_source_readback(row, prefixes[i]) for i in (0, 1)):
        raise ValueError('Both the original plan and withdrawal must be in the received search result')
    frame = row['providerInputs'][1]
    order_frames(frame)
    if hashlib.sha256(target.read_bytes()).hexdigest() != digest:
        raise ValueError('Source recording changed while loading its frame')
    return source, digest, model, frame


def assess_order(rows, model, frame, lookup=False):
    frames = lookup_frames(frame) if lookup else order_frames(frame)
    names = ('current_contract','lookup_aware') if lookup else ('chronological','matched_first')
    arms = (names[0],names[1],names[1],names[0])
    checks = dict(completeTrace=len(rows) == 4, exactOrder=[r.get('id') for r in rows] == list(range(4)),
                  modelAndBudget=bool(model) and all(r.get('model') == model and r.get('requestCount') == r.get('reportedUsageCount') == 1 for r in rows))
    scores = dict.fromkeys(names,0)
    dispositions = []
    for i, row in enumerate(rows):
        if i >= 4: break
        expected_frame = frames[int(arms[i] == names[1])]
        outputs = row.get('providerOutputs', [])
        response = outputs[0] if len(outputs) == 1 else {}
        final = response.get('kind') == 'text' and response.get('stopReason') == 'EndTurn' and response.get('truncated') is False
        calls = response.get('kind') == 'tool_calls' and response.get('count', 0) > 0
        if lookup:
            text, shaped = stability_answer(dict(text=response.get('text')),WINDOW_CASES[0])
            correct = final and shaped and text in WINDOW_CASES[0]['accepted']
        else:
            correct = final and (response.get('text') or '').strip().rstrip('。.') in CONDITIONS[0]['accepted']
        checks[str(i)+'/contract'] = (row.get('arm') == arms[i] and row.get('status') == 'completed' and row.get('toolExecutions') == 0 and
            row.get('providerInputs') == [expected_frame] and row.get('inputTokens', 0) > 0 and row.get('latencyMs', 0) > 0 and (final or calls))
        scores[arms[i]] += int(correct)
        dispositions.append(dict(id=i, arm=arms[i], correct=correct, kind='answer' if final else 'continue_evidence' if calls else 'invalid', text=response.get('text')))
    valid = all(checks.values())
    return dict(status='passed' if valid and sum(scores.values()) == 4 else 'needs_work', comparisonValid=valid,
        checks=checks, scores=scores, dispositions=dispositions,
        usage={k:sum(r.get(k, 0) for r in rows) for k in ('requestCount','inputTokens','outputTokens','latencyMs')})


def run_order_view(live, source_path, lookup=False):
    source, digest, model, frame = order_source(source_path,lookup) if lookup else order_source(source_path)
    payload = dict(version=19 if lookup else 14, live=True, model=model, frame=frame)
    plan = dict(maxProviderCalls=4, softInputStop=30000, order=['A','B','B','A'], toolExecutions=0,
                scope='one public recorded frame; source ordering only, not native protocol replay or new user samples')
    if lookup:
        plan['scope'] = 'first v18 reader, 12 records; completed-host-lookup guidance only, not current-truth assertion or full task execution'
    if not live:
        return dict(status='offline_only', plan=plan, input=payload, sourceSha256=digest)
    owned = Path(tempfile.mkdtemp(prefix='minimal-architecture-', dir=ROOT/'logs')).resolve()
    target = owned/'acceptance.json'
    code_files = ('scripts/prototypes/minimal_architecture.py', 'scripts/context_eval.py',
        'src-tauri/src/commands/message_live_tests/minimal_architecture.rs',
        'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs', 'src-tauri/src/llm/openai_responses.rs')
    report = dict(status='running', comparisonValid=False, usageIncomplete=True, experiment='lookup_contract_v19' if lookup else 'source_order_v14',
        checkedAt=datetime.now(timezone.utc).isoformat(), source=str(source), sourceSha256=digest, plan=plan,
        fixtureSha256=hashlib.sha256(json.dumps(payload,ensure_ascii=False,sort_keys=True).encode()).hexdigest(),
        codeSha256={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in code_files})
    def persist():
        temporary = target.with_suffix('.tmp')
        temporary.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
        temporary.replace(target)
    print(f'MINIMAL_ARCHITECTURE_REPORT {target}', flush=True)
    persist()
    try:
        (owned/'input.json').write_text(json.dumps(payload,ensure_ascii=False),encoding='utf-8')
        result, rows = run_context_worker(owned, WORKER+'_stale', 'MINIMAL_ARCHITECTURE_ROW ', timeout=520)
        report.update(assess_order(rows,model,frame,lookup),results=rows,exitCode=result.returncode)
        report['sourceUnchanged'] = hashlib.sha256((source/'acceptance.json').read_bytes()).hexdigest() == digest
        report['codeUnchanged'] = all(hashlib.sha256((ROOT/p).read_bytes()).hexdigest() == digest for p,digest in report['codeSha256'].items())
        report['usageIncomplete'] = result.returncode != 0 or len(rows) != 4 or any(r.get('reportedUsageCount') != r.get('requestCount') for r in rows)
        report['checks'].update(workerSucceeded=result.returncode == 0,sourceUnchanged=report['sourceUnchanged'],codeUnchanged=report['codeUnchanged'])
        if not all(report['checks'].values()): report.update(status='blocked',comparisonValid=False)
    except BaseException as error:
        report.update(status='interrupted',comparisonValid=False,usageIncomplete=True,interruptionType=type(error).__name__)
        try: persist()
        except Exception as preservation_error: error.add_note(f'Could not preserve order-screen interruption ({type(preservation_error).__name__}).')
        raise
    else: persist()
    return report


def reader_contract(row, case, tool_names=('recall_memories',), max_calls=3):
    frames = row.get('providerInputs', [])
    return (row.get('status') == 'completed' and row.get('arm') == 'current' and
            row.get('coldOpened') is True and row.get('historyMessages') == 0 and
            row.get('archiveUnchanged') is True and 1 <= row.get('requestCount', 0) <= max_calls and
            len(frames) == row.get('requestCount') == len(row.get('providerOutputs', [])) and
            row.get('inputTokens', 0) > 0 and bool(frames) and
            frames[0].get('messages', [{}])[-1].get('content') == case['question'] and all(
                len(f.get('tools', [])) == len(tool_names) and {t.get('name') for t in f.get('tools', [])} == set(tool_names) for f in frames))


def assess_transfer(rows):
    reviews = [r for r in rows if r.get('phase') == 'learn']
    readers = [r for r in rows if r.get('arm') == 'current']
    cases = TRANSFER[-1]['probes']
    checks = {'completeTrace': len(rows) == 11 and len(reviews) == 4 and len(readers) == 7,
              'budget': 0 < sum(r.get('requestCount', 0) for r in rows) <= 25,
              'sameModel': bool(rows) and len({r.get('model') for r in rows}) == 1 and 'luna' in rows[0].get('model', '')}
    for stage in TRANSFER:
        row = next((r for r in reviews if r.get('id') == stage['id']), {})
        prefs = row.get('storedPreferences', [])
        checks[f'{stage["id"]}/review'] = (row.get('status') == 'completed' and row.get('requestCount') == 1 and
            row.get('onlyNewSources') is True and row.get('cursorAdvanced') is True and row.get('inputTokens', 0) > 0 and
            len(row.get('providerInputs', [])) == len(row.get('providerOutputs', [])) == 1 and all(
                source in row['providerInputs'][0].get('messages', [{}])[0].get('content', '') for source in stage['sources']))
        if stage['id'] == 'exception':
            checks['exception/notLearned'] = row.get('newCandidates') == 0 and row.get('preferenceStateUnchanged') is True
        else:
            checks[f'{stage["id"]}/learned'] = all(any(p.get('explicit') and p.get('ready') and
                p.get('content') and p['content'] in source for p in prefs) for source in stage['sources'])
    for case in cases:
        row = next((r for r in readers if r.get('id') == case['id']), {})
        frames = row.get('providerInputs', [])
        tools = '\n'.join(m.get('content', '') for f in frames for m in f.get('messages', []) if m.get('role') == 'tool')
        checks[f'{case["id"]}/contract'] = reader_contract(row, case)
        checks[f'{case["id"]}/projection'] = (len(row.get('visible', [])) == 12 and row.get('omitted', 0) > 0 and
            row.get('archiveRecords') == 12 + row.get('omitted', 0) and not any(
                word in text for text in row.get('visible', []) for word in ('通勤', '汇报', '地铁', '自行车', '打车', '安全风险')))
        checks[f'{case["id"]}/answer'] = (row.get('text') or '').strip() in case.get('accepted', [case['expected']])
        if case.get('readback'):
            checks[f'{case["id"]}/readback'] = any('[habit:' in line and case['readback'] in line for line in tools.splitlines())
        if case.get('exclude_preferences'):
            steps = row.get('steps', [])
            checks['exclusion/explicitlyNarrowed'] = bool(steps) and all(s.get('tool_name') == 'recall_memories' and
                s.get('success') is True and s.get('arguments', {}).get('exclude_preferences') is True for s in steps)
            checks['exclusion/noPreferenceReadback'] = '[habit:' not in tools and '类别:preference' not in tools.lower()
    return {'status': 'passed' if all(checks.values()) else 'needs_work', 'checks': checks,
            'scores': {'current': sum(checks[f'{c["id"]}/answer'] for c in cases)},
            'usage': {k: sum(r.get(k, 0) for r in readers) for k in ('requestCount', 'inputTokens', 'outputTokens')},
            'learningUsage': {k: sum(r.get(k, 0) for r in reviews) for k in ('requestCount', 'inputTokens', 'outputTokens')}}


def stale_source(path, natural=False):
    """Reuse real synthetic learning; never ordinary data or fabricated defaults."""
    source = Path(path).resolve()
    if source.parent != (ROOT / 'logs').resolve() or not source.name.startswith('minimal-architecture-'):
        raise ValueError('Only an owned minimal-architecture recording is allowed')
    files = [source / 'acceptance.json', source / 'state.db']
    if any(p.resolve().parent != source for p in files):
        raise ValueError('Recording files must stay in the owned directory')
    report = json.loads(files[0].read_text(encoding='utf-8'))
    if report.get('experiment') != ('transfer_v5' if natural else 'sequence_v2') or report.get('status') != 'passed' or not all(report.get('checks', {}).values()) or len(report.get('checks', {})) != (39 if natural else 62):
        raise ValueError('Source must be the passed transfer experiment' if natural else 'Source must be the completed sequential learning experiment')
    with contextlib.closing(sqlite3.connect(f'file:{files[1].as_posix()}?mode=ro', uri=True)) as conn:
        for (directory,) in conn.execute("SELECT path FROM projects WHERE kind='project'"):
            if source not in Path(directory).resolve().parents:
                raise ValueError('Source project is outside the owned recording')
        ids = ('seq-0-0', 'seq-1-0', 'seq-3-0') if natural else ('seq-0-1', 'seq-1-0', 'seq-4-0')
        originals = [row[0] for row in conn.execute('SELECT content FROM messages WHERE id IN (?,?,?) ORDER BY id', ids)]
        if len(originals) != 3 or not all(any(word in s for s in originals) for word in (('地铁', '自行车', '打车') if natural else ('乌龙茶', '白茶', '温水'))):
            raise ValueError('Required actual original learning is missing')
    return source, {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files}, report['results'][0]['model']


def assess_stale(rows, live, cases=None, natural=False):
    cases = (NATURAL_CASES if natural else STALE_CASES) if cases is None else cases
    values = ('地铁', '自行车', '打车') if natural else ('乌龙茶', '白茶', '温水')
    diagnostic = next((r for r in rows if r.get('id') == 'lookup_diagnostic'), {})
    checks = {'qualifiedSource': diagnostic.get('status') == 'prepared' and diagnostic.get('manualNoiseReordered') == 36,
              'oldTermIsIncomplete': values[0] in diagnostic.get('oldResult', '') and values[2] not in diagnostic.get('oldResult', '') and (natural or values[1] not in diagnostic.get('oldResult', '')),
              'topicHasCorrection': all(word in diagnostic.get('topicResult', '') for word in values),
              'noTargetInView': len(diagnostic.get('visible', [])) == 12 and diagnostic.get('omitted', 0) > 0 and not any(
                  word in value for value in diagnostic.get('visible', []) for word in (('通勤', '汇报', *values) if natural else ('小榆', *values)))}
    if not live:
        return {'status': 'offline_only' if all(checks.values()) and len(rows) == 1 else 'needs_work', 'checks': checks}
    checks['completeTrace'] = len(rows) == 1 + len(cases)
    checks['boundedCalls'] = 0 < sum(r.get('requestCount', 0) for r in rows) <= 3 * len(cases)
    score = 0
    for case in cases:
        row = next((r for r in rows if r.get('id') == case['id']), {})
        frames = row.get('providerInputs', [])
        tool_text = '\n'.join(m.get('content', '') for f in frames for m in f.get('messages', []) if m.get('role') == 'tool')
        checks[f'{case["id"]}/contract'] = reader_contract(row, case)
        checks[f'{case["id"]}/answer'] = (row.get('text') or '').strip() in case.get('accepted', [case['expected']])
        score += checks[f'{case["id"]}/answer']
        if case.get('readback'):
            checks[f'{case["id"]}/readback'] = any('[habit:' in line and case['readback'] in line for line in tool_text.splitlines())
    return {'status': 'passed' if all(checks.values()) else 'needs_work', 'checks': checks, 'scores': {'current': score},
            'usage': {k: sum(r.get(k, 0) for r in rows) for k in ('requestCount', 'inputTokens', 'outputTokens')}}


def run_stale(live, source_path, followup=None, natural=False, temporal=False, temporal_followup=False, budget=False):
    source, hashes, model = stale_source(source_path, natural or temporal or budget)
    if (temporal or budget) and not live:
        return {'status': 'offline_only', 'input': budget_projection(model) if budget else temporal_projection(model, temporal_followup),
                'plan': {'learningCalls': 2, 'readerCases': 12 if budget else 6, 'maxProviderCalls': 38 if budget else 20,
                         'softInputStop':100000 if budget else 60000, 'softTotalStop':120000 if budget else None}}
    owned = Path(tempfile.mkdtemp(prefix='minimal-architecture-', dir=ROOT / 'logs')).resolve()
    temporal_stages = TEMPORAL_FOLLOWUP if temporal_followup else TEMPORAL
    cases = [c for s in (BUDGET if budget else temporal_stages) for c in s['probes']] if temporal or budget else NATURAL_CASES if natural else stale_cases(followup)
    payload = budget_projection(model) if budget else temporal_projection(model, temporal_followup) if temporal else {'version': 6 if natural else 4 if followup else 3, 'live': live, 'model': model, 'cases': [{k: c[k] for k in ('id', 'question')} for c in cases]}
    report = {'status': 'blocked', 'experiment': 'budget_pair_v9' if budget else 'temporal_v8' if temporal_followup else 'temporal_v7' if temporal else 'natural_transfer_v6' if natural else f'stale_option_{followup or "v3"}', 'checkedAt': datetime.now(timezone.utc).isoformat(),
              'source': str(source), 'sourceSha256': hashes, 'maxProviderCalls': 38 if budget else 20 if temporal else 3 * len(cases),
              'fixtureSha256': hashlib.sha256(json.dumps({'input': payload, 'cases': cases}, ensure_ascii=False, sort_keys=True).encode()).hexdigest(),
              'codeSha256': {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in (
                  'scripts/prototypes/minimal_architecture.py', 'src-tauri/src/commands/message_live_tests/minimal_architecture.rs',
                  'src-tauri/src/agent/extensions/memory.rs', 'src-tauri/src/agent/support/adaptive_constraints.rs',
                  'src-tauri/src/agent/conversation/prompt.rs', 'src-tauri/src/agent/conversation/runner.rs',
                  'src-tauri/src/fts.rs',
                  'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs', 'src-tauri/src/llm/openai_responses.rs',
                  'src-tauri/src/commands/memory.rs', 'src-tauri/src/agent/support/evolution.rs')}}
    target = owned / 'acceptance.json'
    print(f'MINIMAL_ARCHITECTURE_REPORT {target}', flush=True)
    with contextlib.closing(sqlite3.connect(f'file:{(source / "state.db").as_posix()}?mode=ro', uri=True)) as original, contextlib.closing(sqlite3.connect(owned / 'state.db')) as copied:
        original.backup(copied)
    (owned / 'input.json').write_text(json.dumps(payload, ensure_ascii=False), encoding='utf-8')
    try:
        result, rows = run_context_worker(owned, WORKER + ('_sequence' if temporal or budget else '_stale'), 'MINIMAL_ARCHITECTURE_ROW ', timeout=1750 if budget else 1000 if temporal else 520)
        report.update(assess_budget(rows) if budget else assess_temporal(rows, temporal_stages) if temporal else assess_stale(rows, live, cases, natural), results=rows, exitCode=result.returncode)
        report['sourceUnchanged'] = all(hashlib.sha256((source / name).read_bytes()).hexdigest() == digest for name, digest in hashes.items())
        report['usageIncomplete'] = result.returncode != 0 or any(r.get('status') not in ('prepared', 'completed') for r in rows)
        if result.returncode or not report['sourceUnchanged']:
            report['status'] = 'blocked'
    finally:
        target.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return report


def without_preferences(system):
    return re.sub(r'## Learned interaction defaults\n.*?(?=\n\n## |\Z)', '', system, flags=re.S).strip()


def assess(rows):
    learn = next((r for r in rows if r.get('id') == 'learn'), {})
    stored = learn.get('storedPreferences', [])
    checks = {
        'actualLearning': learn.get('status') == 'completed' and learn.get('requestCount') == 1 and
        all(any(p.get('explicit') and p.get('ready') and phrase in p.get('content', '') for p in stored)
            for phrase in ('小沐', '今天也辛苦了')),
        'completeTrace': len(rows) == 9 and len({(r.get('arm'), r.get('id')) for r in rows}) == 9,
        'boundedCalls': 0 < sum(r.get('requestCount', 0) for r in rows) <= 25,
    }
    pairs = {}
    scores = {arm: 0 for arm in ARMS}
    for case in CASES:
        pair = {}
        for arm in ARMS:
            row = next((r for r in rows if r.get('id') == case['id'] and r.get('arm') == arm), {})
            frames = row.get('providerInputs', [])
            tool_text = '\n'.join(m.get('content', '') for f in frames for m in f.get('messages', []) if m.get('role') == 'tool')
            correct = (row.get('text') or '').strip() == case['expected']
            checks[f'{arm}/{case["id"]}/contract'] = bool(frames) and all(
                [t.get('name') for t in f.get('tools', [])] == ['recall_memories'] for f in frames
            ) and frames[0].get('messages', [{}])[-1].get('content') == case['question'] and (
                row.get('status') == 'completed' and row.get('coldOpened') is True and
                row.get('historyMessages') == 0 and row.get('archiveUnchanged') is True and
                1 <= row.get('requestCount', 0) <= 3 and len(frames) == row.get('requestCount') and
                len(row.get('providerOutputs', [])) == len(frames) and row.get('inputTokens', 0) > 0)
            checks[f'{arm}/{case["id"]}/answer'] = correct
            if arm == 'bounded_recall':
                checks[f'{arm}/{case["id"]}/projection'] = len(row.get('visible', [])) == 12 and row.get('omitted', 0) > 0 and (
                    row.get('archiveRecords') == 12 + row.get('omitted', 0)) and not any(
                        phrase in text for text in row.get('visible', []) for phrase in ('小沐', '今天也辛苦了', '记得歇一歇'))
                if case['source_needed']:
                    checks[f'{arm}/{case["id"]}/readback'] = case['expected'].rstrip('。') in tool_text
            else:
                checks[f'{arm}/{case["id"]}/projection'] = row.get('omitted') == 0 and row.get('archiveRecords') == len(row.get('visible', [])) and row.get('archiveRecords', 0) > 12
            if correct:
                scores[arm] += 1
            pair[arm] = frames[0] if frames else {}
        full, bounded = (pair[a] for a in ARMS)
        systems = [[m.get('content', '') for m in f.get('messages', []) if m.get('role') == 'system'] for f in (full, bounded)]
        pairs[case['id']] = bool(systems[0] and systems[1]) and (
            [without_preferences(s) for s in systems[0]] == [without_preferences(s) for s in systems[1]] and
            full.get('tools') == bounded.get('tools'))
    checks['pairedActualInputs'] = all(pairs.values())
    return {'status': 'passed' if all(checks.values()) else 'needs_work', 'checks': checks, 'scores': scores,
            'pairs': pairs, 'usage': {arm: {k: sum(r.get(k, 0) for r in rows if r.get('arm') == arm)
                                            for k in ('requestCount', 'inputTokens', 'outputTokens')} for arm in ARMS},
            'learningUsage': {k: learn.get(k, 0) for k in ('requestCount', 'inputTokens', 'outputTokens')}}


def assess_sequence(rows):
    reviews = [r for r in rows if r.get('phase') == 'learn']
    readers = [r for r in rows if r.get('arm') in SEQUENCE_ARMS]
    checks = {'completeTrace': len(rows) == 18 and len(reviews) == 4 and len(readers) == 14,
              'budget': 0 < sum(r.get('requestCount', 0) for r in rows) <= 46}
    for stage in SEQUENCE:
        if stage['sources']:
            review = next((r for r in reviews if r.get('id') == stage['id']), {})
            checks[f'{stage["id"]}/review'] = (review.get('status') == 'completed' and review.get('requestCount') == 1 and
                review.get('onlyNewSources') is True and review.get('cursorAdvanced') is True and review.get('inputTokens', 0) > 0)
            prefs = review.get('storedPreferences', [])
            required = {'initial': ('小榆', '乌龙茶'), 'correction': ('白茶',), 'conditional': ('晚上', '温水')}.get(stage['id'], ())
            checks[f'{stage["id"]}/learned'] = all(any(p.get('explicit') and p.get('ready') and word in p.get('content', '') for p in prefs) for word in required)
            if stage['id'] == 'exception':
                checks['exception/notLearned'] = review.get('newCandidates') == 0 and review.get('preferenceStateUnchanged') is True
        for case in stage['probes']:
            pair = []
            for arm in SEQUENCE_ARMS:
                row = next((r for r in readers if r.get('id') == case['id'] and r.get('arm') == arm), {})
                frames = row.get('providerInputs', [])
                tools = [f.get('tools', []) for f in frames]
                checks[f'{arm}/{case["id"]}/contract'] = (row.get('status') == 'completed' and row.get('stage') == stage['id'] and
                    row.get('coldOpened') is True and row.get('archiveUnchanged') is True and row.get('historyMessages') == 0 and
                    1 <= row.get('requestCount', 0) <= 3 and len(frames) == row.get('requestCount') and
                    len(row.get('providerOutputs', [])) == len(frames) and row.get('inputTokens', 0) > 0 and
                    bool(frames) and frames[0].get('messages', [{}])[-1].get('content') == case['question'] and
                    all([t.get('name') for t in ts] == ['recall_memories'] for ts in tools))
                checks[f'{arm}/{case["id"]}/answer'] = (row.get('text') or '').strip() == case['expected']
                cap = 12 if arm == 'view12' else 6
                checks[f'{arm}/{case["id"]}/projection'] = (0 < row.get('archiveRecords', 0) < 50 and
                    len(row.get('visible', [])) == min(cap, row.get('archiveRecords', 0)) and
                    row.get('omitted') == row.get('archiveRecords', 0) - len(row.get('visible', [])))
                if case.get('source_needed'):
                    checks[f'{arm}/{case["id"]}/readback'] = bool(frames) and case['expected'] in '\n'.join(
                        m.get('content', '') for f in frames for m in f.get('messages', []) if m.get('role') == 'tool') and not any(
                            case['expected'] in v for v in row.get('visible', []))
                pair.append(frames[0] if frames else {})
            systems = [[without_preferences(m.get('content', '')) for m in f.get('messages', []) if m.get('role') == 'system'] for f in pair]
            checks[f'{case["id"]}/paired'] = bool(systems[0] and systems[1]) and systems[0] == systems[1] and pair[0].get('tools') == pair[1].get('tools')
    usage = {arm: {k: sum(r.get(k, 0) for r in readers if r.get('arm') == arm) for k in ('requestCount', 'inputTokens', 'outputTokens')} for arm in SEQUENCE_ARMS}
    qualifies = (all(checks.values()) and usage['view12']['inputTokens'] > 0 and
                 usage['view6']['inputTokens'] <= usage['view12']['inputTokens'] * 0.9 and
                 usage['view6']['requestCount'] <= usage['view12']['requestCount'])
    return {'status': 'passed' if all(checks.values()) else 'needs_work', 'checks': checks,
            'scores': {arm: sum(checks[f'{arm}/{c["id"]}/answer'] for s in SEQUENCE for c in s['probes']) for arm in SEQUENCE_ARMS},
            'usage': usage, 'learningUsage': {k: sum(r.get(k, 0) for r in reviews) for k in ('requestCount', 'inputTokens', 'outputTokens')},
            'candidateQualifiesForHoldout': qualifies,
            'configurationDecision': 'retain_production_12_pending_independent_evidence'}


def run(live, sequence=False, transfer=False):
    plan = {'arms': ARMS, 'learningCalls': 1, 'readerCases': 4, 'maxProviderCalls': 25,
            'softInputStop': 120000, 'noise': '36 manually admitted records, not model learned',
            'scope': 'real core loop; no full Main/coordinator, scheduler, multi-day or other-model claim'}
    stages = TRANSFER if transfer else SEQUENCE
    payload = sequence_projection(stages, 5 if transfer else 2) if sequence or transfer else projection()
    cases = [c for s in stages for c in s['probes']] if sequence or transfer else CASES
    if sequence:
        plan.update(arms=SEQUENCE_ARMS, learningCalls=4, readerCases=7, maxProviderCalls=46,
                    simulatedTime='dated sources + owned review cooldown only; through-message cursor preserved')
    if transfer:
        plan.update(arms=('current',), learningCalls=4, readerCases=7, maxProviderCalls=25, softInputStop=80000,
                    simulatedTime='source dates; manually admitted noise reordered, not actual elapsed days')
    if not live:
        return {'status': 'offline_only', 'plan': plan, 'cases': cases, 'input': payload}
    owned = Path(tempfile.mkdtemp(prefix='minimal-architecture-', dir=ROOT / 'logs')).resolve()
    report = {'status': 'blocked', 'experiment': 'transfer_v5' if transfer else 'sequence_v2' if sequence else 'combined_v1', 'plan': plan, 'checkedAt': datetime.now(timezone.utc).isoformat(),
              'fixtureSha256': hashlib.sha256(json.dumps({'input': payload, 'cases': cases}, ensure_ascii=False, sort_keys=True).encode()).hexdigest(),
              'codeSha256': {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in (
                  'scripts/prototypes/minimal_architecture.py', 'scripts/context_eval.py',
                  'src-tauri/src/commands/message_live_tests/minimal_architecture.rs',
                  'src-tauri/src/agent/extensions/memory.rs', 'src-tauri/src/agent/support/adaptive_constraints.rs',
                  'src-tauri/src/agent/conversation/prompt.rs', 'src-tauri/src/agent/conversation/runner.rs',
                  'src-tauri/src/fts.rs',
                  'src-tauri/src/commands/message_live_tests/agent_benchmarks.rs')}}
    target = owned / 'acceptance.json'
    print(f'MINIMAL_ARCHITECTURE_REPORT {target}', flush=True)
    (owned / 'input.json').write_text(json.dumps(payload, ensure_ascii=False), encoding='utf-8')
    try:
        result, rows = run_context_worker(owned, WORKER + '_sequence' if sequence or transfer else WORKER, 'MINIMAL_ARCHITECTURE_ROW ', timeout=1000 if transfer else 1750 if sequence else 1100)
        report.update(assess_transfer(rows) if transfer else assess_sequence(rows) if sequence else assess(rows), results=rows, exitCode=result.returncode)
        report['usageIncomplete'] = result.returncode != 0 or any(r.get('status') != 'completed' for r in rows)
        if result.returncode:
            report['status'] = 'blocked'
    finally:
        target.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--replay', action='store_true')
    parser.add_argument('--live', action='store_true')
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument('--sequence', action='store_true')
    modes.add_argument('--transfer', action='store_true', help='fresh commute/report topics; same core, single 12-record arm')
    modes.add_argument('--stale-reader', type=Path, help='reuse a passed v2 fixture; test old-option queries, no relearning')
    modes.add_argument('--clock-view', type=Path, help='v10 format-only screen using an immutable v9 future-prefix snapshot')
    modes.add_argument('--stability-reader', type=Path, help='v11 two fixed cold trials on the two immutable v9 prefixes')
    modes.add_argument('--source-reader', type=Path, help='v12 reuse Main conversation readers for withdrawal/history, two cold trials')
    modes.add_argument('--order-view', type=Path, help='v14 ABBA single-inference source-order screen on a completed synthetic v13')
    modes.add_argument('--lookup-view', type=Path, help='v19 ABBA one-inference lookup-contract screen on the first v18 reader')
    parser.add_argument('--scenario', choices=('original', 'conditions', 'daily', 'counter', 'counter-view', 'view-budget', 'lookup-guards'), default='original', help='source-reader: conditions, cards, profiles, window pair or v20 completed-lookup guards')
    parser.add_argument('--stale-followup', nargs='?', const='v4', choices=('v4', 'v5', 'v6'), help='fixed v4/v5 questions or known v5 replay under v6 contract; requires --stale-reader')
    parser.add_argument('--natural', action='store_true', help='new ordinary questions; --stale-reader must be a passed transfer recording')
    parser.add_argument('--temporal', action='store_true', help='future effective date and withdrawal; reuses a passed transfer recording')
    parser.add_argument('--temporal-followup', action='store_true', help='fresh v8 dates, values and wording under the revised extraction contract')
    parser.add_argument('--budget-pair', action='store_true', help='v9 low/medium readers on the same actual learned prefixes')
    args = parser.parse_args()
    if args.scenario != 'original' and not args.source_reader:
        parser.error('--scenario requires --source-reader')
    if (args.clock_view or args.stability_reader or args.source_reader or args.order_view or args.lookup_view) and any((args.stale_followup, args.natural, args.temporal, args.temporal_followup, args.budget_pair)):
        parser.error('--clock-view/--stability-reader/--source-reader cannot use another followup')
    if args.stale_followup and not args.stale_reader:
        parser.error('--stale-followup requires --stale-reader')
    if args.natural and (not args.stale_reader or args.stale_followup):
        parser.error('--natural requires --stale-reader and cannot use --stale-followup')
    if args.temporal and (not args.stale_reader or args.stale_followup or args.natural):
        parser.error('--temporal requires --stale-reader and cannot use another followup')
    if args.temporal_followup and not args.temporal:
        parser.error('--temporal-followup requires --temporal')
    if args.budget_pair and (not args.stale_reader or args.stale_followup or args.natural or args.temporal or args.temporal_followup):
        parser.error('--budget-pair requires --stale-reader and cannot use another followup')

    def evaluate():
        if args.lookup_view:
            return run_order_view(args.live,args.lookup_view,lookup=True)
        if args.order_view:
            return run_order_view(args.live, args.order_view)
        if args.source_reader:
            return run_stability(args.live, args.source_reader, sources=True, scenario=args.scenario)
        if args.stability_reader:
            return run_stability(args.live, args.stability_reader)
        if args.clock_view:
            return run_clock_view(args.live, args.clock_view)
        if args.stale_reader:
            return run_stale(args.live, args.stale_reader, args.stale_followup, args.natural, args.temporal, args.temporal_followup, args.budget_pair)
        return run(args.live, args.sequence, args.transfer)

    if args.replay:
        report = evaluate()
        print(json.dumps({k: v for k, v in report.items() if k != 'results'}, ensure_ascii=False))
        return 0 if report['status'] in ('passed', 'offline_only') else 2
    index, report = 0, None
    if args.order_view:
        cases = [CONDITIONS[0]]
    elif args.source_reader:
        cases = stability_cases(True, args.scenario)
    elif args.stability_reader:
        cases = STABILITY
    elif args.clock_view:
        cases = BUDGET[0]['probes'][:2]
    elif args.budget_pair:
        cases = [c for s in BUDGET for c in s['probes']]
    elif args.temporal:
        stages = TEMPORAL_FOLLOWUP if args.temporal_followup else TEMPORAL
        cases = [c for s in stages for c in s['probes']]
    elif args.stale_reader:
        cases = NATURAL_CASES if args.natural else stale_cases(args.stale_followup)
    else:
        stages = TRANSFER if args.transfer else SEQUENCE
        cases = [c for s in stages for c in s['probes']] if args.sequence or args.transfer else CASES
    while True:
        print('\x1b[2J\x1b[H', end='')
        print(json.dumps({'case': cases[index], 'mode': 'live' if args.live else 'offline',
                          'arms': ('A','B','B','A') if args.order_view else ('stable',) if args.stability_reader or args.source_reader else ('iso','local') if args.clock_view else ('low','medium') if args.budget_pair else ('current',) if args.stale_reader or args.transfer else SEQUENCE_ARMS if args.sequence else ARMS, 'result': report and report.get('scores')}, ensure_ascii=False, indent=2))
        action = input('[r] run once  [n] next  [q] quit > ').strip().lower()
        if action == 'q': return 0
        if action == 'n': index = (index + 1) % len(cases)
        if action == 'r' and report is None: report = evaluate()


if __name__ == '__main__':
    sys.stdout.reconfigure(encoding='utf-8')
    raise SystemExit(main())
