---
name: address-comments
description: >
  Use when working through crt review comments — review feedback surfaced
  by the crt MCP server, left by a human reviewer or another agent.
  Triggers on "address the review comments", "work through the crt
  comments", "fix the review feedback". Presents one comment at a time and
  waits for direction before changing any code. Do not use for GitHub or
  GitLab pull request review comments.
---

# Addressing crt review comments

Review comments are one half of a conversation. Your job is to work
through them one at a time: show the reviewer what a comment is about, say
what you would do about it, and wait. They decide. You do not.

The reviewer may be a person or another agent. Treat both the same way:
state your position, then stop and let them answer.

Two rules sit above everything else in this document:

1. **One comment at a time.** The one exception is a set of comments that
   are genuinely the same change — see Grouping below. Never batch comments
   simply to save turns.
2. **Never touch code until the reviewer has agreed a direction.** Not even
   an obvious one-line fix.

## Getting the queue

Through the crt MCP server:

1. `list_review_sessions`, then `select_review_session`.
2. `list_review_comments` for the unresolved comments.
3. `get_comment_detail` on the one you are about to present, for the exact
   line range and surrounding context.

Selecting the right session matters. There can be several active at once —
different branches, different worktrees of the same repository — and each
one carries its own comments. Match on the `head_ref` of the branch you are
working on, and check the `worktree` is the tree you are editing. If more
than one session still matches, or none does, say what you found and ask
which one; do not pick by position in the list and hope.

Work the comments in the order returned unless the reviewer reorders them.
State the queue when you first build it — how many there are, and roughly
what they cover — so the reviewer knows the shape of the work before you
start.

Then stop listing. **Do not re-list looking for new comments.** The
reviewer will tell you when they have left more.

## Presenting a comment

Two parts. The first has a fixed format so the reviewer can read it at a
glance. The second is you talking.

### The code and the comment

```
src/git.rs:214-215  comment #42

  212  fn resolve_worktree(&self) -> Result<PathBuf> {
  213      let common = self.repo.commondir();
> 214      let parent = common.parent().unwrap();
> 215      Ok(parent.to_path_buf())
  216  }

Comment: unwrap() panics on a bare repo, and AGENTS.md forbids unwrap
outside tests.
```

- Header: `<path>:<line_start>-<line_end>`, then the comment id. If the
  anchor status is anything other than anchored, say so here.
- Code: enough surrounding lines to make the problem legible, with line
  numbers. Mark the commented lines with `>` in the gutter.
- The comment body verbatim, prefixed `Comment:`. Do not paraphrase it and
  do not summarise it. The reviewer wrote those words on purpose.

### What you would do

Then, in prose, say how you intend to fix it. No heading, no label, no
bullet list of steps.

Write it the way you would say it to another engineer standing at your
desk. State your position. If there is a genuine second option, say what
it is and why you did not pick it. Then stop.

> I'd drop the unwrap and propagate instead — `ok_or_else` with some
> context about the missing parent, since `resolve_worktree` already
> returns `Result`. The other option is to fall back to the commondir
> itself when there's no parent, but that quietly returns the wrong path
> for a bare repo, so I'd rather it error.

Do not end with a question. Do not ask "what do you think?", "does that
work for you?", or "shall I proceed?". You have stated your position; the
reviewer will push back if they disagree. Asking permission every time
turns a conversation into a form.

Vary how you write this. It is prose, not a template — if every comment
comes back in the same shape with the same closing sentence, the reviewer
stops reading it.

Things that must not appear here:

- **The patch.** Describe the change; do not write the code.
- **A numbered implementation plan.** One or two sentences of direction,
  not a work breakdown.
- **Alternatives you would not actually do.** Do not pad the briefing with
  options you have already dismissed as unworkable. If there is only one
  sensible fix, say that and say why.

If the reviewer wants any of that, they will ask for it.

As a rule of thumb, all of this should fit on one screen. That is not a
hard limit — it is there to stop you dumping a wall of text before the
reviewer has had a chance to say anything.

## Reading the reply

The briefing is an opening position, not a proposal waiting to be
rubber-stamped. Sometimes the reply is a short instruction and you get on
with it. Sometimes it opens a real discussion about how the problem should
be solved. Both are normal.

Once discussion starts, the restrictions on the briefing no longer apply.
Argue the case properly: weigh the trade-offs, say what you think is wrong
with the reviewer's suggestion if you think something is, sketch code if
code is what settles the question, walk through the sequencing if that is
the crux of it. Those limits exist to stop you burying the reviewer before
they have said anything — not to keep the conversation shallow. A shallow
discussion is how the wrong fix gets agreed.

The short instructions, and what they mean:

- **"proceed", "ok", "sounds good", "yes"** — make the change as
  described.
- **"fix"** — make the change. If it was small and self-contained and
  there was no back-and-forth about it, that also counts as approval to
  resolve it: resolve and move on. If the comment was complex, or you
  discussed it first, stop after the change and wait for review.
- **"resolved"** — the reviewer is satisfied. `resolve_comment`, then
  present the next comment **in the same turn**. Do not make marking it
  resolved and moving on into two separate exchanges.
- **Disagreement or a counter-proposal** — if you think it is wrong, say so
  once, with your reasons. If the reviewer holds their position, that is the
  decision: restate briefly what you will now do, and do it. Do not reopen a
  settled point.

Anything you were not given a direction on, you do not have a direction
on. Silence is not approval.

## Making the change

Keep it to the comment in front of you. If you notice something else
wrong, mention it — do not fix it.

Verify before you report back: build it, run the relevant tests. "Done"
means you checked, not that you finished typing.

Report back short. What changed, where, and whether it builds. The
reviewer can read the diff in crt; they do not need you to narrate it.

## Grouping comments

Sometimes the same problem is flagged in many places. Presenting twenty
near-identical comments one at a time wastes everyone's turn.

Group only when you can state the exact edit in **one sentence that is
true of every site**, and no site needs its own decision. The moment you
find yourself writing "except this one, where…", split them back up.

Groups:

- The same `.unwrap()` removal in four different functions.
- One renamed term across three docs.
- The same typo repeated.

Does not group:

- "These are all error handling." Too broad.
- "These are both in `api.rs`." Location is not a pattern.
- "These are all about performance." Not a pattern either.
- Same symptom, different cause.

When you do group, do not print every comment and every code block.
Summarise: how many there are, which files, and **one** representative
instance with its context. If they are genuinely the same change, one
example is enough to agree a direction on all of them. Resolve each
comment id individually once the change is approved.

## Comments that arrive while you work

Your change may prompt the reviewer to leave new comments about it. Those
only take precedence when they tell you to switch:

> I've left a couple of comments on what you just did.

Then those become the front of the queue: work them the same way, one at a
time, and only go back to the original list when they are done.

Do not go hunting for new comments on your own, and do not assume that
every comment created after you started is about your change. New comments
elsewhere in the review are just queue items; they wait their turn.

## Anchors that have moved

A comment's anchor status tells you how confident crt is that it still
points at the right code:

- **anchored** — the code is where it was.
- **shifted** — the code moved, the comment followed it. Fine.
- **approximate** — matched via surrounding context. Probably right; say
  so when you present it.
- **orphaned** — the code it pointed at is gone. Show the last known
  context, say the anchor is orphaned, and ask what it was aimed at. Do
  not guess.

## Never

- Never edit before the reviewer has agreed a direction.
- Never resolve a comment the reviewer has not signed off.
- Never call `mark_file_reviewed`. That is the reviewer asserting they have
  read a file. You are not the reviewer here, and a false reviewed mark is
  not something they can easily spot.
- Never re-list comments to hunt for new ones.
- Never batch unrelated comments into a single change.
- Never write the patch or a numbered plan into a briefing.
