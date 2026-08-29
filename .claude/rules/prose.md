# Prose

Both rules below apply to every file in this repository: Markdown, Rust doc comments and line comments, PR bodies, and commit messages.

## Never join clauses with a dash

No ` — `, no `——`, no ` -- `.

Write two sentences instead, or use a colon when the second half explains the first.
A comma with a conjunction covers most of the rest.

A dash inside a CLI flag, a numeric range, or an ASCII rule is not prose and stays.
So does one inside a quoted error message or a URL.

## Never hard-wrap prose

**One sentence is one line.**
Break after a sentence-ending `.` in English and after `。` in Japanese.

The reader's viewport decides where a sentence wraps.
A diff then shows the sentence that changed rather than the six lines that shifted under it.

**The unit is the sentence, not the paragraph.**
Folding a whole paragraph onto one line reaches 400 characters and reads worse than the wrapping it replaced.

A `.` inside a code span, a number, a version or an abbreviation does not end a sentence.
Neither does one inside `**bold**`: breaking there strands the closing `**` at the head of the next line, where it renders as literal asterisks.

A comment block holding a list, a table, a fence or indented code keeps its line breaks: those carry meaning.

`rustfmt` does not reflow a comment's interior, so a long comment line passes `cargo fmt --check` unchanged.
`max_width` governs code, not the prose inside `//`, `///` and `//!`.

## Fixing these mechanically is banned

A script that unwraps paragraphs cut a parenthetical in half at a `。` inside brackets, and six agents given the same instructions reproduced it in three files.
Edit prose by hand, and check the shape of the result: a line whose brackets do not close is a break in the wrong place, and no whitespace-stripped comparison can see it.

## The two checks feed each other

Unwrapping a block joins its lines and can put two sentences on the result; splitting a sentence shortens lines and can drop a block into a wrap detector's window.
A fix reported at one line can surface a new report elsewhere in the same file.
That is progress, not regression.
