#!/usr/bin/env python3
"""Keep the Getting Started Execution/WorkUnit/integration order aligned."""

from pathlib import Path


document = Path(__file__).resolve().parents[1] / "docs/getting-started.md"
text = document.read_text(encoding="utf-8")
section = text.split("## 8. Review, validation, gates, integration, and closure", 1)[1]

if 'if [[ "$STATUS" != "completed" ]]; then' not in text:
    raise SystemExit("Getting Started must stop unless the exact Execution completed")

markers = [
    "### 1. Confirm the exact Execution completed",
    "### 2. Read the current WorkUnit and version",
    'WORK_UNIT="$(request GET "/work-units/$WORK_UNIT_ID")"',
    "### 3. Mark that WorkUnit completed",
    "'{expected_version:$version,status:\"completed\"}'",
    "### 4. Integrate the exact Execution result",
    'request POST "/work-units/$WORK_UNIT_ID/integrations"',
    "### 5. Inspect the integration outcome",
    'case "$INTEGRATION_OUTCOME" in',
    "### 6. Read TaskLifecycle and Gates before any merge",
    'request GET "/tasks/$TASK_ID/lifecycle"',
    'request GET "/tasks/$TASK_ID/gates"',
]

positions = [section.find(marker) for marker in markers]
if any(position < 0 for position in positions):
    missing = [marker for marker, position in zip(markers, positions) if position < 0]
    raise SystemExit(f"Getting Started order contract missing: {missing}")
if positions != sorted(positions):
    raise SystemExit("Getting Started steps are out of order")

for outcome in ("success", "conflict", "failed", "rejected", "running"):
    if outcome not in section:
        raise SystemExit(f"Getting Started must handle integration outcome {outcome}")

print("Getting Started WorkUnit order: PASS")
