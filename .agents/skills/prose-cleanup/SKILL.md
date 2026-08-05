---
name: prose-cleanup
description: "Use this skill after an LLM has written or edited prose — docs, READMEs, design notes, comments — to strip stale references, prescriptive step-by-step voice, redundant justification, detail pitched below the document's level of abstraction, cross-page restatement, and stylistic tics. Invoke before committing LLM-authored documentation, or when asked to tighten, trim, shorten, or clean up writing."
---

# Prose cleanup

LLM prose accretes. Each edit adds context the model assumes the reader lacks, hedges the previous edit, and leaves a trail of what changed. Nothing is ever removed. This is the removal pass.

The goal is a document that states what is true, in the present tense, in a declarative voice. Sentences should be fluent, not clipped: the length problem is almost never at the sentence level. It is the same idea in four places, each with a paragraph explaining why it is being said, and mechanism written into a document whose job was to name it and move on.

Never add a claim during a cleanup pass. If cutting something would lose information you cannot verify elsewhere, ask instead of deleting.

## Process

1. Read the whole document, then every other document that touches its subject. Duplication is the largest source of length and is invisible one file at a time.
2. Grep distinctive phrases and proper nouns across the full doc set, including agent guides like `AGENTS.md` / `CLAUDE.md`. The same fact in three files means picking one home.
3. Fix altitude before style: decide what belongs in each section at all, then cut within what survives.
4. Delete first, rewrite second.
5. Verify links and anchors, then read the result start to finish. Brevity does not license clipped prose.

## Altitude

A document that summarizes others sits above them, and a summary is not a compressed copy. It states what each part is and what follows from it. Mechanism, thresholds, enumerated rules, and the argument for any of them belong one level down, in the document that owns the topic.

It is hard to see because every sentence in the offending paragraph is true and on topic.

**Cut the support, keep the claim.** The shape is *claim → because → therefore*. The claim earns its place, the consequence usually does, the "because" rarely does. It is often the best sentence of the three, which is why it was written.

> The cache is invalidated on write. That is the storage engine's constraint rather than a choice — readers hold a snapshot until they close, so an in-place update would be invisible to anything already reading. Stale reads are therefore impossible.

> The cache is invalidated on write. Stale reads are therefore impossible.

A short "because" survives where the fact would otherwise look arbitrary. A clause that argues for a choice, appeals to a project value, or weighs a tradeoff does not.

**Drop topics rather than compressing them.** A summary is not a table of contents in prose. Working through the source document's headings produces third and fourth paragraphs that exist for coverage. Delete rather than tighten when the paragraph introduces a noun the section never uses again, when nothing above it depends on it, when it is a rule set or an option list, or when tightening leaves a sentence carrying no more than the heading and the link already do. Summarizing sections settle at two or three paragraphs; check the last one first.

**Say what a thing is, not what it would take.** Conditional framing ("if X becomes the binding constraint, the answer is Y") and stakes-setting ("which is the budget every other decision is made against") are altitude drift inside a sentence. State the fact and let the reader weigh it. Keep a number a later claim depends on; cut the ones that only illustrate it.

**Brevity is not compression into jargon.** Folding an idea into a metaphor, a house term, or a callback to a numbered invariant shortens a sentence and narrows it to readers who already know the answer — and a summary is read before the documents it summarizes. Where a cut leaves a sentence that only parses with context the reader does not have yet, name the noun.

## What to cut

### Stale references

Documents describe the design as it is. Git holds what it was.

Search for: `used to`, `no longer`, `previously`, `originally`, `once planned`, `is now`, `was replaced`, `at time of writing`, `these days`, `recently`. Also the transitional voice — `it is now mostly a description of`, `this used to need a separate mechanism` — which describes the current state by reference to the state it replaced.

Distinguish two things that look alike. "We rejected X because Y" is current design rationale and belongs beside the decision it explains. "We used to say Z about X and that is no longer true" is changelog and belongs in the commit.

### Prescriptive voice

A document describes the design as it stands, not the work of building it.

Search for: `step 1`, `first,` / `then,` / `finally,`, `TODO`, `TBD`, `we will`, `we need to`, `in the future`, `for now`, `eventually`, `phase 1`, `next steps`, `should be implemented`, `is planned`.

Also the imperative form, which carries no tense marker and so matches none of the above: `delete this`, `delete it once`, `remove when`, `replace this`, `can go away`, `until there are`, `once there are`, `for the time being`, `placeholder`, `temporary`, `scaffolding`, `stopgap`.

Numbered procedure is right for a runbook, a migration guide, or a skill. It is wrong for a design document, where the numbering implies an order of construction that goes stale the moment anything is built out of order. Sequence that is genuinely part of the design — a protocol exchange, a state machine — is described as sequence, in the present tense.

`TODO` and `TBD` belong in the tracker or the code. If something is undecided, say what is undecided and what constrains the answer: "the chunk size is bounded below by signature cost and above by the MTU" is a fact about the design; "TODO: pick a chunk size" is a note to self.

Present tense throughout. If something is not true yet, either it is a rule about future work — phrase it as a rule — or it does not belong in the document.

### Scaffolding, and instructions to remove it

A file that exists only until something replaces it is not part of the design. Documenting it promotes a placeholder to an architectural element, and the description is guaranteed to be wrong within a month. Cut it; if the transience is worth recording, it belongs in a comment next to the code, stated in the present.

The test is whether a sentence would still be true after the work everyone expects to happen.

This cuts the other way too. Do not write a document *around* the current state — "there is only one screen so far", "the store is not wired up yet". Absence of an implementation is not a property of the design.

### Commentary about the document

The document should not narrate itself. Delete the frame, keep the fact.

Cut: "this section covers", "note that", "as mentioned above", "before we get into", "the key insight is", "it is worth being precise here", "worth stating so nobody goes looking for it".

### Defensive justification

LLMs anticipate objections nobody raised, then rebut them. Keep reasoning that stops a future reader from re-litigating a settled decision. Cut reasoning that defends the writing, reassures the reader, or argues that a choice was reasonable. State the choice and the constraint that forced it.

### Sections about things that do not exist

Hypotheticals do not get headings. A heading like "What X would change" is section-length speculation about unbuilt work; if a future option constrains present decisions, it is a sentence where that constraint lives. Also suspect: "Alternatives considered", "Future work", "What this would look like if…", and any section describing a benchmark or migration that has not run.

The exception is a reserved seam stated as a rule, which constrains code being written today. Keep it phrased as a rule, not as a scenario.

### Restatement

Symptoms, in rough order of frequency:

- A section opens with a summary of itself, then says it again.
- A section closes with a recap, usually with a flourish.
- The same fact appears in the agent guide, the overview, and the topic doc, phrased three ways.
- A parenthetical re-explains a term the document defined two paragraphs earlier.

The fix is one canonical statement in the document that owns the topic. Everywhere else gets a clause and a link. Agent guides state the rule and link out; they do not carry the reasoning.

### Decisions logs

A decision-and-rationale table at the foot of a document duplicates the document. Each row either restates a rule the prose already carries, or holds the only copy of a reason that belongs beside the rule it justifies. Editing the prose leaves the table asserting the old design, and the table is the copy nobody updates.

Attach the reason to the decision, in the section that owns it. A rejected alternative is worth a sentence there — what was rejected, and the one thing that ruled it out — rather than a row somewhere else. Deleting a log is not a delete-only operation: check every row for rationale that appears nowhere in the prose, and rehome it before the table goes.

### Style tics

| Tic | Fix |
| --- | --- |
| Em dash as a drum roll before a reveal | A period. Sometimes a comma. |
| "not X, but Y" · "X is not a Y; it is a Z" | Say what it is. |
| Tricolon: "no relays, no servers, no internet" | Pick the one that carries information. |
| Aphoristic closing sentence | Delete. The last sentence of a section is the likeliest cut in the document. |
| "This is the whole of X" · "that is the point" · "which is what makes this work" | Delete. |
| Bold lead-in on every paragraph | Keep only where paragraphs are parallel items in a list. |
| Hedges: "essentially", "effectively", "arguably", "in practice", "of course" | Delete, or commit to the claim. |
| Rhetorical question followed by its answer | Keep the answer. |
| "It is not just about X" | Delete. |

Em dashes are not banned. A parenthetical pair is fine. A single em dash setting up a punchline at the end of a sentence is the tell.

## What to keep

Cutting is not the goal; brevity is. A stub costs more than it saves, and a second pass that re-derives deleted reasoning is expensive.

- Numbers, kind numbers, field names, exact behaviors, names of things — in the document that owns them. A summary keeps the ones a later claim depends on.
- Why a decision was made, once, in the document that owns it.
- Prohibitions, with the reason attached. "Do not do X" without a reason gets undone.
- Links, especially anchor links that give a claim its full argument elsewhere.

Do not merge sections just to reduce heading count. Headings are how the document is navigated.

## Check

- Word count dropped.
- Summarizing sections state what a thing is and what follows from it; mechanism, rule lists and arguments appear only in the document that owns them.
- No paragraph survives because its subject deserved coverage.
- No sentence got shorter by becoming a metaphor or an in-house allusion.
- Every sentence is in the present tense, and would still be true after the work everyone expects to happen.
- Deleted headings are not linked from anywhere: grep the anchor slug across the repo before finishing.
- Nothing was asserted that the previous version did not assert.
