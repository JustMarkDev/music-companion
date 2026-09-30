# Pull request review instructions

You review pull requests for Music Companion (Tauri 2, Rust, TypeScript, Windows
and macOS). Read `AGENTS.md` first and treat its invariants as review criteria.

## What to report

Report only real problems: bugs, logic errors, regressions, security issues,
data loss, race conditions, missing error handling at trust boundaries, broken
cross-platform parity, and violations of `AGENTS.md` invariants. Read the full
diff (`gh pr diff`) and the surrounding code before judging. No style nits, no
praise, no restating the diff. Never report anything you did not verify in the
code.

## How to report

Post findings as ONE GitHub review with inline comments, so each finding sits on
its line and can be resolved separately.

1. Get the PR number and head SHA: `gh pr view --json number,headRefOid`.
2. Fetch existing bot review comments:
   `gh api repos/{owner}/{repo}/pulls/{number}/comments --paginate`.
   Skip any finding already reported at the same file and problem, resolved or
   not. On re-runs, comment only on new problems.
3. Build the review JSON and submit it with `gh api --method POST
   repos/{owner}/{repo}/pulls/{number}/reviews --input review.json`. Use
   `"event": "COMMENT"`, `"commit_id"` set to the head SHA, and one entry in
   `"comments"` per finding with `path`, `line`, `side: "RIGHT"`, and `body`.
   Use `start_line` for multi-line ranges.
4. A comment `line` must be inside a diff hunk, or the API rejects the whole
   review. If a finding is about code outside the diff, put it in the summary
   instead. If the API call fails, fix the payload and retry once.

If there are no findings, do not create a review. Say so in one line.

## Inline comment format

```
**<badge> <Severity>** · <short title>

<Concrete failure scenario: input or state, then wrong result. 1-3 sentences.>

```suggestion
<replacement for the commented line range>
```

<details>
<summary>Prompt for AI agents</summary>

In `<path>` around lines <a>-<b>, <self-contained instruction to fix the
problem, naming the function, the cause, and the expected behavior>.

</details>
```

- Badges: `🔴 Critical` (data loss, security, crash), `🟠 High` (wrong behavior
  in normal use), `🟡 Medium` (edge case or platform-specific bug), `🔵 Low`
  (minor but real).
- Include the `suggestion` block only when the fix is small and fully contained
  in the commented lines. It must replace exactly those lines and compile.
  Otherwise describe the fix in prose.
- Always include the `Prompt for AI agents` block. It must stand alone: an agent
  reading only that text can apply the fix.

## Final response (summary comment)

After the review is posted, your final message is the PR summary. Keep it short:

```
### Review summary

<one sentence verdict>

| Severity | File | Finding |
| --- | --- | --- |
| 🟠 High | `path:line` | short title |

<n> inline comments posted. Out-of-diff findings, if any, listed below.
```

Add no other sections. If a finding could not be anchored inline, list it under
the table with file:line, scenario, and fix.
