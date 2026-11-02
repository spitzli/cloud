#!/usr/bin/env python3
"""Fail release readiness closed until actual Blacksmith CI evidence exists."""
import json
from pathlib import Path
import re
import sys

EVIDENCE = Path(__file__).with_name('blacksmith-readiness-2026-11-01.json')


def blockers(evidence):
    failures = list(evidence.get('blockers', []))
    if evidence.get('status') != 'qualified':
        failures.append('Blacksmith CI is not qualified; skipped jobs do not count as passed checks.')
    if evidence.get('blacksmithInstalled') is not True:
        failures.append('Blacksmith organization integration is missing.')
    region = evidence.get('qualifiedRegion')
    if region not in {'us-east', 'us-west', 'eu-central', 'eu-west'} or not evidence.get('regionProof'):
        failures.append('Actual Blacksmith organization region has no reviewed provider proof.')
    isolation = evidence.get('builder', {})
    if not all(isolation.get(field) is True for field in ['qualified', 'fallbackDisabled']) or isolation.get('backend') not in {'blacksmith-managed', 'qualified-rootless'} or not isolation.get('proof'):
        failures.append('Blacksmith build backend and disabled fallback have no qualification proof.')
    expected = evidence.get('repositories', [])
    jobs = evidence.get('successfulChecks', [])
    if not expected:
        failures.append('Expected release repositories are missing.')
    for repository in expected:
        checks = [job for job in jobs if job.get('repository') == repository]
        if not checks or not all(job.get('conclusion') == 'success'
                and job.get('region') == region
                and job.get('runnerLabel') == 'blacksmith-4vcpu-ubuntu-2404'
                and re.fullmatch(r'[0-9a-f]{40}', job.get('sourceSha', ''))
                and re.fullmatch(r'https://github\.com/' + re.escape(repository) + r'/actions/runs/[0-9]+/job/[0-9]+', job.get('jobUrl', ''))
                for job in checks):
            failures.append('Actual successful Blacksmith source check missing: ' + repository)
    return failures


def main():
    try:
        failures = blockers(json.loads(EVIDENCE.read_text()))
    except (OSError, ValueError, TypeError, AttributeError):
        print('Blacksmith readiness evidence is missing or invalid.', file=sys.stderr)
        return 1
    for failure in failures:
        print('BLOCKED: ' + failure, file=sys.stderr)
    if failures:
        return 1
    print('Recorded Blacksmith qualification is complete; review the linked provider/job evidence for the release source.')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
