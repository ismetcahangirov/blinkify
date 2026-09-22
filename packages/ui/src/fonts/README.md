# Bundled typefaces

The files in this directory ship inside the application. They are committed
rather than installed, because "bundled with the application, never fetched at
runtime" (#16) is a property of the shipped bytes, and a package manager is one
more thing that can be between the user and a working interface on first launch.

They are also small — 68 KB for both — and change approximately never. Git
history is permanent (`CLAUDE.md` section 5), so this is a deliberate decision
rather than an accident: two font files, versioned once, not a corpus.

## Provenance

| File                                    | Family                 | Version                            | Source                                                                                                      |
| --------------------------------------- | ---------------------- | ---------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `inter-latin-wght-normal.woff2`         | Inter, variable `wght` | `@fontsource-variable/inter@5.3.0` | `https://cdn.jsdelivr.net/npm/@fontsource-variable/inter@5.3.0/files/inter-latin-wght-normal.woff2`         |
| `jetbrains-mono-latin-400-normal.woff2` | JetBrains Mono, 400    | `@fontsource/jetbrains-mono@5.3.0` | `https://cdn.jsdelivr.net/npm/@fontsource/jetbrains-mono@5.3.0/files/jetbrains-mono-latin-400-normal.woff2` |

SHA-256, so a replacement is a decision rather than a drift:

```
3100e775e8616cd2611beecfa23a4263d7037586789b43f035236a2e6fbd4c62  inter-latin-wght-normal.woff2
14425ba9c695763c1547f48a206b7aa60350a33ae23de09f0407877f3fcd89eb  jetbrains-mono-latin-400-normal.woff2
```

## Licence

Both are SIL Open Font License 1.1 — permissive, and permissive for embedding,
which is what `CLAUDE.md` section 10 requires. The licence text accompanies the
fonts as the OFL demands: `LICENSE-Inter.txt` and `LICENSE-JetBrainsMono.txt`.
Both are listed in [`THIRD_PARTY.md`](../../../../THIRD_PARTY.md).

Neither family is redistributed under its reserved name modified in any way.

## Replacing a font

1. Replace the file, the row above and the hash above.
2. Run `pnpm test`. `fonts.test.ts` parses the WOFF2 and asserts the timecode
   family advances every digit and every timecode separator identically. A
   proportional face fails there rather than in someone's eyes three months
   later, which is the entire reason the test reads the binary.
3. Update `THIRD_PARTY.md` if the licence or the version changed.
