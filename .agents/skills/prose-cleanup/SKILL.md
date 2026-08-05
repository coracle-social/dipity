---
name: prose-cleanup
description: "Use this skill after an LLM has written or edited prose — docs, READMEs, design notes, comments — to strip stale references, prescriptive step-by-step voice, redundant justification, cross-page restatement, and stylistic tics. Invoke before committing LLM-authored documentation, or when asked to tighten, trim, shorten, or clean up writing."
---

# Prose cleanup

LLM prose accretes. Each edit adds context the model assumes the reader lacks, hedges the previous edit, and leaves a trail of what changed. Nothing is ever removed. This is the removal pass.

The goals are simple, direct, and clear, in a declarative voice: the document states what is true of the design, in the present tense. Sentences should be fluent, not clipped — the length problem is almost never at the sentence level. It is the same idea stated in four places, each time with a paragraph explaining why it is being stated.

Never add a claim during a cleanup pass. If cutting something would lose information you cannot verify elsewhere, ask instead of deleting.

## Process

1. Read the whole document, then read every other document that touches its subject. Duplication is the largest source of length and is invisible one file at a time.
2. Grep distinctive phrases and proper nouns from the edited sections across the full doc set, including agent guides like `AGENTS.md` / `CLAUDE.md`. The same fact in three files means picking one home.
3. Delete first, rewrite second. There is no point polishing sentences that should not exist.
4. Verify links and anchors, then read the result start to finish.

## What to cut

### Stale references

Documents describe the design as it is. Git holds what it was.

Search for: `used to`, `no longer`, `previously`, `originally`, `once planned`, `is now`, `was replaced`, `at time of writing`, `these days`, `recently`.

> A fourth reason used to sit here — that iroh drags in Rust and uniffi — and it no longer applies. The three reasons above are unaffected and are sufficient on their own; what changed is that "it costs a language" is not one of them any more. Recorded rather than deleted, because an argument that quietly stops being true is worse than one that is openly retired.

Cut the whole paragraph. The three reasons are already listed above it.

Distinguish two things that look alike. "We rejected X because Y" is current design rationale and belongs in a decisions log. "We used to say Z about X and that is no longer true" is changelog and belongs in the commit.

Same for the transitional voice — `it is now mostly a description of`, `this used to need a separate mechanism`, `now an app-level gate`. Describe the current state without reference to the state it replaced.

### Prescriptive voice

A document describes the design. It is not a plan for building it, and not a set of instructions to follow.

Search for: `step 1`, `first,` / `then,` / `finally,`, `TODO`, `TBD`, `we will`, `we need to`, `in the future`, `for now`, `eventually`, `phase 1`, `next steps`, `should be implemented`, `is planned`.

Also search for the imperative form, which carries no tense marker and so matches none of the above: `delete this`, `delete it once`, `remove when`, `remove this once`, `replace this`, `can go away`, `until there are`, `once there are`, `for the time being`, `placeholder`, `temporary`, `scaffolding`, `stopgap`.

> **Step 3.** Once the handshake completes, we then need to send the auth challenge. In the future this will also carry a generation counter.

> Reachability is exchanged after the Noise handshake and carries a monotonic generation counter.

Numbered procedure is right for a runbook, a migration guide, or a skill. It is wrong for a design document, where the numbering implies an order of construction that the reader has no reason to care about and that goes stale the moment anything is built out of order. Sequence that is genuinely part of the design — a protocol exchange, a state machine — is described as sequence, in the present tense, not as steps to perform.

`TODO` and `TBD` belong in the tracker or the code, not in prose. If something is undecided, say what is undecided and what constrains the answer, in the present tense: "the chunk size is not fixed; it is bounded below by signature cost and above by the MTU." That is a fact about the design. "TODO: pick a chunk size" is a note to self.

Present tense throughout. `will be`, `is going to`, `is planned to` become `is`. If it is not true yet, either it is a rule about future work — phrase it as a rule — or it does not belong in the document.

### Scaffolding, and instructions to remove it

Two failures wear the same sentence, and the grammatical one hides the substantive one.

> `src/App.svelte` renders the token scales in both themes so they can be checked by eye. It is a design system reference; delete it once there are real screens.

The surface problem is `delete it once` — an imperative aimed at the future, which the searches above miss because it marks no tense.

The real problem is that the sentence is in the document at all. A design document describes the design. A file that exists only until something replaces it is not part of the design; it is the current state of the tree, which git already holds and which the document is guaranteed to describe wrongly within a month. Worse, documenting it promotes a placeholder to an architectural element — a reader now believes the app has a design system reference screen, and that was never a decision anyone made.

Cut it. If the transience is worth recording, it is a fact about the code and belongs in a header comment next to the code, stated in the present: "Scaffolding. Renders the token scales from `app.css`."

The test is whether a sentence would still be true after the work everyone expects to happen. If not, the document is describing a moment rather than a design.

This cuts the other way too. Do not write a document *around* the current state — "there is only one screen so far", "the store is not wired up yet". State what the design is; absence of an implementation is not a property of the design.

### Commentary about the document

The document should not narrate itself.

> It is worth being precise here about what it would buy the sync layer, because the answer is narrower than "everything gets faster" — and knowing which parts do not move is what stops it being treated as the fix.

> Worth stating so nobody goes looking for it: …

Delete the frame, keep the fact. Also cut: "this section covers", "note that", "as mentioned above", "before we get into", "the key insight is".

### Defensive justification

LLMs anticipate objections nobody raised, then rebut them.

> This is the same tradeoff every background-signing app makes, and it is a real reduction against a stolen-and-still-powered phone.

Keep reasoning that stops a future reader from re-litigating a settled decision. Cut reasoning that defends the writing, reassures the reader, or argues that a choice was reasonable. State the choice and the constraint that forced it; that is the argument.

### Sections about things that do not exist

A heading like **"What L2CAP would change"** is section-length speculation about unbuilt work. Hypotheticals do not get headings. If a future option constrains present decisions, it is a sentence where that constraint lives, or a row in the decisions log.

Also suspect: "Alternatives considered" that restates the decisions log, "Future work", "What this would look like if…".

The exception is a reserved seam stated as a rule — "do not add a transport that needs a rendezvous server" constrains code being written today. Keep it, phrased as a rule, not as a scenario.

### Restatement

Symptoms, in rough order of frequency:

- A section opens with a summary of itself, then says it again.
- A section closes with a recap, usually with a flourish.
- The same fact appears in the agent guide, the overview, and the topic doc, phrased three ways.
- A parenthetical re-explains a term the document defined two paragraphs earlier.

The fix is one canonical statement in the document that owns the topic. Everywhere else gets a clause and a link. Agent guides state the rule and link out; they do not carry the reasoning.

### Style tics

| Tic | Fix |
| --- | --- |
| Em dash as a drum roll before a reveal | A period. Sometimes a comma. |
| "not X, but Y" · "X is not a Y; it is a Z" | Say what it is. |
| Tricolon — "no relays, no servers, no internet" | Pick the one that carries information. |
| Aphoristic closing sentence | Delete. The last sentence of a section is the likeliest cut in the document. |
| "This is the whole of X" · "that is the point" · "which is what makes this work" | Delete. |
| Bold lead-in on every paragraph | Keep only where paragraphs are parallel items in a list. |
| Hedges: "essentially", "effectively", "arguably", "in practice", "of course" | Delete, or commit to the claim. |
| Rhetorical question followed by its answer | Keep the answer. |
| "It is not just about X" | Delete. |

Em dashes are not banned. A parenthetical pair is fine. A single em dash setting up a punchline at the end of a sentence is the tell.

## What to keep

Cutting is not the goal; brevity is. A stub costs more than it saves, and a second pass that re-derives deleted reasoning is expensive.

- Numbers, kind numbers, field names, exact behaviors, names of things.
- Why a decision was made — once, in the document that owns it.
- Prohibitions, with the reason attached. "Do not do X" without a reason gets undone.
- Links, especially anchor links that give a claim its full argument elsewhere.

Do not merge sections just to reduce heading count. Headings are how the document is navigated.

## Check

- Word count dropped. If it did not, the pass did not happen.
- Every sentence is in the present tense and describes what is true, not what was true or what someone should do next.
- Every sentence would still be true after the work everyone expects to happen. Anything describing a placeholder, or telling a reader when to delete something, is out.
- Deleted headings are not linked from anywhere: grep the anchor slug across the repo before finishing.
- Nothing was asserted that the previous version did not assert.
- Read it through. Fluency is the constraint that brevity operates under, not the other way around.
