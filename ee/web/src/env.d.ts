/// <reference types="@rsbuild/core/types" />

// Ambient declarations for the non-TypeScript assets rsbuild lets us import -- `*.css` (the
// side-effect imports in main.tsx and the React Flow pages), images, and fonts. rsbuild ships
// these already, so this file only has to point TypeScript at them; without the reference,
// `noUncheckedSideEffectImports` rejects every `import "./index.css"` as an untyped module.
