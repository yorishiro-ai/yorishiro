# Prose

Use present tense for repository prose, including Markdown, comments, PR bodies, and commit messages.

## Never join clauses with a dash

No ` — `, no `——`, no ` -- `.

Write two sentences instead, or use a colon when the second half explains the first.
A comma with a conjunction covers most of the rest.

A dash inside a CLI flag, a numeric range, or an ASCII rule is not prose and stays.
So does one inside a quoted error message or a URL.

## Never hard-wrap prose

**One sentence is one line.**
Break after a sentence-ending `.`, `?` or `!` in English, and after `。` in Japanese.

Let the viewport wrap the line and keep each sentence on its own line so diffs show only the sentences you edit.

A `.` inside a code span, a number, a version or an abbreviation does not end a sentence.
Neither does one inside `**bold**`: breaking there strands the closing `**` at the head of the next line, where it renders as literal asterisks.

A comment block holding a list, a table, a fence or indented code keeps its line breaks: those carry meaning.

`rustfmt` does not reflow a comment's interior, so a long comment line passes `cargo fmt --check` unchanged.
`max_width` governs code, not the prose inside `//`, `///` and `//!`.

Edit prose by hand and check that parentheses and brackets stay intact.
