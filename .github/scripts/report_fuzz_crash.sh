#!/usr/bin/env bash
# Report a deep-fuzz crasher onto ONE pinned issue.
#
# A new issue per weekly run would bury the first finding under duplicates of
# itself, since a crasher stays in the corpus and keeps being found until it is
# fixed. So the first crash opens an issue and later ones comment on it -- the
# drift-alarm pattern #102 asks for.
#
# Called only from .github/workflows/deep-fuzz.yml, on failure of the fuzz step.
set -euo pipefail

LABEL="fuzz-crash"
TITLE="Deep fuzz: crasher in parse_email"

# `find` on a missing directory exits 1, and under `pipefail` that ends the
# script before it can report anything -- the one case where reporting matters
# most is a crash whose artifact directory never got created.
mkdir -p fuzz/artifacts

count=$(find fuzz/artifacts -type f | wc -l | tr -d ' ')
files=$(find fuzz/artifacts -type f | sort | sed 's|^|- `|; s|$|`|')
[ -n "$files" ] || files="- (none on disk; see the log)"

# The tail carries libFuzzer's own diagnosis: the panic message, the dedup
# token, and where it wrote the reproducer.
excerpt=$(tail -n 40 fuzz-output.log 2>/dev/null || echo "(no log captured)")

# The smallest crasher inline, if small enough to be useful. Saves whoever picks
# this up from downloading an artifact to see what the input was.
smallest=$(find fuzz/artifacts -type f -exec ls -S {} + | tail -n 1)
if [ -n "$smallest" ] && [ "$(wc -c < "$smallest")" -le 2048 ]; then
  reproducer=$(printf 'Smallest crasher, base64 (`%s`, %s bytes):\n\n```\n%s\n```' \
    "$(basename "$smallest")" "$(wc -c < "$smallest" | tr -d ' ')" \
    "$(base64 < "$smallest")")
else
  reproducer="The crashers are in the run artifact; none was small enough to inline."
fi

body=$(cat <<EOF
The scheduled deep fuzz run found **${count}** crasher(s).

Run: ${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}
Artifact: \`deep-fuzz-${GITHUB_RUN_ID}\` (crashers plus the full log)

Files:

${files}

${reproducer}

<details><summary>Last 40 lines of the fuzzer output</summary>

\`\`\`
${excerpt}
\`\`\`

</details>

---

If this says \`fuzz canary\`, it is the reporting path being tested on purpose
and nothing is wrong -- close it.

Otherwise: minimize (\`cargo fuzz tmin parse_email <crasher>\`), commit the
minimized input as a regression fixture so the finding stays found, then fix it.
EOF
)

# --label fails on a label that does not exist yet, and the first crash is
# exactly when it will not.
gh label create "$LABEL" \
  --color B60205 \
  --description "Found by the scheduled deep fuzz run" >/dev/null 2>&1 || true

existing=$(gh issue list --label "$LABEL" --state open --limit 1 \
  --json number --jq '.[0].number // empty')

if [ -n "$existing" ]; then
  echo "commenting on existing #${existing}"
  gh issue comment "$existing" --body "$body"
else
  echo "opening a new issue"
  gh issue create --title "$TITLE" --label "$LABEL" --body "$body"
fi
