# Project Instructions

These instructions apply to all work in this repository.

## Technical English

Use ASD-STE100 Simplified Technical English for project documentation and user-facing technical text.

- Use approved words when they have the required meaning.
- Use one term for one meaning. Do not use a synonym only to add variety.
- Use short, direct sentences.
- Give one instruction in each sentence when possible.
- Use the active voice when the actor is known.
- Put conditions before the action when this order prevents ambiguity.
- Define an abbreviation or acronym before its first use.
- Keep necessary software names, protocol names, code symbols, and quoted original text unchanged.
- If strict ASD-STE100 vocabulary cannot express a software concept correctly, use the necessary technical term. Define it in plain language.

## Quality Writing

Follow William Zinsser's four principles of quality writing.

1. **Simplicity**: Use familiar words and direct sentence structures.
2. **Brevity**: Remove words, sentences, and sections that do not add useful information.
3. **Clarity**: State the actor, action, condition, and result without ambiguity.
4. **Humanity**: Write in a natural and respectful voice. Help the reader understand and act.

When principles conflict, preserve technical accuracy first. Then use the simplest and shortest clear form.

## Ghidra Reverse Engineering

Keep confirmed reverse-engineering knowledge in the shared Ghidra project.

- Read existing Ghidra names and comments before you trace the same code again.
- Rename a function, variable, parameter, global, structure, or field when evidence supports its meaning.
- Add a concise comment at each important function, branch, field, or instruction.
- In a comment, record useful protocol facts such as a byte offset, bit mask, active level, data width, or observed effect.
- Use a confidence-qualified name when the general purpose is known but the exact meaning is not confirmed.
- Do not replace a useful name or comment unless stronger evidence shows that it is incorrect.
- If you correct a prior interpretation, update its Ghidra name and comment. Record why the interpretation changed.
- Save the Ghidra project changes before you finish the task.
- Also write important results in the applicable project analysis document. The document does not replace Ghidra names and comments.

## Automated Test Scope

Write automated tests only for protocol implementations and server implementations.

- Do not add automated tests for runtime hooks, capture code, configuration, storage, graphics, input, user-interface behavior, quality-of-life features, foreign function interfaces, build logic, evidence files, or reverse-engineering helpers.
- For code outside a protocol or server implementation, use formatting, compilation, static analysis, and focused live verification.
- If a task changes both protocol code and non-protocol code, test only the protocol behavior.
- Do not keep a test that a task added outside the permitted scope. Remove it before you finish the task.
- Do not add a regression test only because a defect occurred. The protocol-and-server restriction still applies.
