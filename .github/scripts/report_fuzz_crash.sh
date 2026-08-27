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
# Also capped: 45 crashers is a plausible haul, and an unbounded list is how a
# report grows past what GitHub will accept.
MAX_LISTED=20
files=$(find fuzz/artifacts -type f | sort | head -n "$MAX_LISTED" \
  | sed 's|^|- `|; s|$|`|')
[ -n "$files" ] || files="- (none on disk; see the log)"
if [ "$count" -gt "$MAX_LISTED" ]; then
  files="${files}
- ... and $((count - MAX_LISTED)) more, in the artifact"
fi

# The tail carries libFuzzer's dedup token and where it wrote the reproducer.
#
# Capped by BYTES as well as lines, because a line has no bound: an assertion
# message can be as large as the values it prints, and this harness compares whole
# header maps. Forty such lines came to hundreds of kilobytes, the body went to
# `gh` as a command-line argument, and the report died with "Argument list too
# long" -- on the first real finding, so 45 crashers went unreported.
EXCERPT_BYTES=3000
excerpt=$(tail -n 40 fuzz-output.log 2>/dev/null | tail -c "$EXCERPT_BYTES" \
  || echo "(no log captured)")
[ -n "$excerpt" ] || excerpt="(no log captured)"

# ...but not reliably the panic message. A Rust panic prints the location and
# then the message on the following line, and 40 lines of stack frames can push
# both out of the tail -- which is exactly what happened on the first drill. So
# pull it out explicitly: it is the single most useful line in the log.
# Capped for the same reason as the excerpt: a panic message is as large as the
# values it prints, and this harness prints whole header maps. Uncapped, this
# single field reached 40 KB on the first real finding.
PANIC_BYTES=1500
panic=$(grep -m1 -A2 "panicked at" fuzz-output.log 2>/dev/null \
  | head -c "$PANIC_BYTES" || true)
if [ -z "$panic" ]; then
  # No Rust panic: a timeout, an OOM, or a signal. libFuzzer's own summary says
  # which.
  panic=$(grep -m1 -E "^(SUMMARY|==[0-9]+==ERROR)" fuzz-output.log 2>/dev/null \
    | head -c "$PANIC_BYTES" || true)
fi
[ -n "$panic" ] || panic="(the log records no panic or summary line)"

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

\`\`\`
${panic}
\`\`\`

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

# Written to a file rather than passed as an argument: a single argv string is
# capped by the kernel (MAX_ARG_STRLEN, 128 KiB), and this body is assembled from
# fuzzer output whose size is not ours to choose. GitHub also rejects a body over
# 65536 characters, so it is trimmed to fit with a line saying so.
body_file=$(mktemp)
trap 'rm -f "$body_file"' EXIT
printf '%s\n' "$body" > "$body_file"

MAX_BODY=60000
if [ "$(wc -c < "$body_file")" -gt "$MAX_BODY" ]; then
  head -c "$MAX_BODY" "$body_file" > "${body_file}.trimmed"
  printf '\n\n_Report trimmed to %s bytes; the full log is in the artifact._\n' \
    "$MAX_BODY" >> "${body_file}.trimmed"
  mv "${body_file}.trimmed" "$body_file"
  echo "report trimmed to $MAX_BODY bytes"
fi

if [ -n "$existing" ]; then
  echo "commenting on existing #${existing}"
  gh issue comment "$existing" --body-file "$body_file"
else
  echo "opening a new issue"
  gh issue create --title "$TITLE" --label "$LABEL" --body-file "$body_file"
fi
