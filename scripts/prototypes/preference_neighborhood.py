#!/usr/bin/env python3
"""THROWAWAY: can saved-source neighbors recover a later keyword-free correction?

python scripts/prototypes/preference_neighborhood.py          # offline TUI
python scripts/prototypes/preference_neighborhood.py --replay # curated views only
Real store + same saved Luna screen (<=8 requests; not full Main or learning):
python scripts/context_eval.py --suite continuity --experiment preference_neighborhood --live

The TUI exposes curated admission, not language understanding. Actual screening
reuses the Rust store's source checks and current lexical reader. Neither path
reads ordinary data or changes production recall. No new dependency/persistence.
"""
import argparse
import json
from pathlib import Path

FIXTURE = Path(__file__).resolve().parents[1] / 'fixtures/context_continuity.json'


def view(case, expanded):
    """Pure selection over the frozen, manually declared saved statements."""
    saved = case['receipt']['stored_indices']
    groups = case['receipt']['source_sessions']
    hits = [i for i in saved if case['query'].lower() in case['history'][i]['content'].lower()]
    seed = hits[0]
    selected = {seed}
    if expanded:
        group = [i for i in saved if groups[i] == groups[seed]]
        position = group.index(seed)
        selected.update(group[max(0, position - 1):position + 2])
    return [{'source': f'source-{i:04}', 'quote': case['history'][i]['content'],
             'match': 'lexical' if i == seed else 'same_source_neighbor'} for i in sorted(selected)]


def episode_view(case):
    return {'episodes':[{'sourceGroup':'same-user-session','statements':view(case,True)}]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--replay', action='store_true')
    parser.add_argument('--grouping', action='store_true', help='separate fixed flat/episode cases')
    args = parser.parse_args()
    cases = json.loads(FIXTURE.read_text(encoding='utf-8'))['preference_grouping_cases' if args.grouping else 'preference_cases']
    if args.replay:
        for case in cases:
            print(json.dumps({'id':case['id'],'baseline':view(case,args.grouping),'proposal':episode_view(case) if args.grouping else view(case,True)},ensure_ascii=False))
        return
    index, expanded = 0, False
    while True:
        print('\033[2J\033[H', end='')
        case = cases[index]
        print('\033[1mSaved-preference neighborhood — prototype, not production\033[0m')
        print(json.dumps({'case':case['id'],'query':case['query'],'expanded':expanded,
                          'candidates':episode_view(case) if args.grouping and expanded else view(case,args.grouping or expanded)},ensure_ascii=False,indent=2))
        print('\n\033[2mManually nominated records; no extraction, inference or persistence.\033[0m')
        key = input('[n] next case  [v] toggle view  [q] quit\n> ').strip().lower()
        if key == 'q': return
        if key == 'n': index = (index + 1) % len(cases)
        if key == 'v': expanded = not expanded


if __name__ == '__main__':
    main()
