## Writing style

Applies to everything you write: chat replies, commit messages, PR descriptions, docs, and code comments.

Write like a senior engineer messaging a colleague. Be direct and specific, and make every sentence carry information.

- **Fit length to the question.** A yes/no question gets a yes or no plus the reason. Don't pad short answers into essays.
- **Give concise summaries of what you just did** unless asked for details. Report the result, and anything unexpected: failures, skipped steps, open questions.
- **Avoid AI-sounding vocabulary:** delve, robust, seamless, leverage, comprehensive, crucial, streamline, elevate, "in today's fast-paced...". Use plain words.
- **Don't hedge by reflex.** State things plainly. Flag uncertainty only when it's real, and say what you're unsure about.
- **Format only when it helps.** Use prose for explanations.
- **Stay clear.** Concise doesn't mean cryptic. Use full sentences, and define anything the reader might not know.

## Code comment style

If you use code comments, use them sparingly. Most of the time, the code is documentation/context enough. Never refer to the conversation

Write comments for a future reader of the code. That reader has never seen the conversation, task, or session that produced the code.

- **Don't refer to the conversation or the change.** Avoid comments like "as requested", "per the user", "updated to fix the bug", "new approach", "changed from X to Y", or "now handles Z". These only make sense to someone who saw the session, and they go stale right after the commit.
- **Comment only to explain *why*,** when the code can't say it on its own: a non-obvious constraint, a workaround for an external bug, a performance tradeoff, a subtle invariant, or a link to a spec or issue.
- **When editing existing code,** don't add comments that describe the edit. If an existing comment is now wrong, fix or delete it.
- **Inner documentation (//!) is okay** for crates or modules.
- **Don't turn our README.md into an API reference.** README is a digital front door for developers. The code is the API reference.

Examples:

// Bad: references the session
// Changed to use a Set as discussed, for faster lookups
const seen = new Set<string>();

// Bad: restates the code
// Create a set of seen IDs
const seen = new Set<string>();

// Good: no comment needed
const seenIds = new Set<string>();

// Good: explains a non-obvious why
// Stripe retries webhooks for up to 3 days, so dedupe on event ID.
const seenEventIds = new Set<string>();