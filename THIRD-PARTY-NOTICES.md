# Third-party notices

Neptune's own code is licensed under the [MIT license](LICENSE). Third-party
material retains the licenses and attribution below.

## Vendored skills

The development skills live in `.agents/skills/`; `.claude/skills/` links to the
same files. [skills-lock.json](skills-lock.json) records their direct sources.
The license files below are verbatim copies from the linked upstream revisions;
those revisions identify the license sources, not the installed skill versions.

| Material | Upstream license source | Local license notice |
| --- | --- | --- |
| `.agents/skills/better-ui/` | [Jakub Krehel's skills](https://github.com/jakubkrehel/skills/blob/267330e1adfc66a718fb65fa6918c1f06d0a689e/LICENSE) | [MIT](licenses/skills/jakubkrehel-skills-MIT.txt) |
| `.agents/skills/emil-design-eng/` | [Emil Kowalski's skills](https://github.com/emilkowalski/skills/blob/d16ebe60d09a5ba2afcb7054ede9d0a10c9f6128/LICENSE) | [MIT](licenses/skills/emilkowalski-skills-MIT.txt) |
| `.agents/skills/rust-best-practices/` skill guide and adaptations | [Apollo GraphQL's skills](https://github.com/apollographql/skills/blob/222dfc07720227bd0f330bd1b45a9741b1c97bee/LICENSE) | [MIT](licenses/skills/apollographql-skills-MIT.txt) |
| All other skill directories listed in `skills-lock.json` | [Matt Pocock's skills](https://github.com/mattpocock/skills/blob/d81f3a183412e71a5b1e84ca21bc1a35eea03a60/LICENSE) | [MIT](licenses/skills/mattpocock-skills-MIT.txt) |
| Visual examples and placement guidance adapted in `.agents/skills/pr/SKILL.md` | [HumanLayer's skills](https://github.com/humanlayer/skills/blob/ca7c8088db69e315a8b2deea43820270457f8f3c/LICENSE) | [MIT](licenses/skills/humanlayer-skills-MIT.txt) |
| Handbook material in `.agents/skills/rust-best-practices/references/chapter_01.md` through `chapter_09.md` | [Apollo GraphQL's Rust Best Practices Handbook](https://github.com/apollographql/rust-best-practices/blob/eb485a5ddb68e0ded3d79549e994f63ec0a6f6c0/LICENSE) | [Apache-2.0](licenses/skills/apollographql-rust-best-practices-Apache-2.0.txt) |

The `pr` skill adapts Dex Horthy and HumanLayer's
[`show-me`](https://github.com/humanlayer/skills/blob/ca7c8088db69e315a8b2deea43820270457f8f3c/plugins/show-me/skills/show-me/SKILL.md)
material for pull request diffs. Its [credits](.agents/skills/pr/CREDITS.md)
identify that adaptation. The Rust reference chapters adapt Apache-2.0 handbook
material for skill use and include local edits; each chapter carries a source
and modification notice. Apollo's skill adaptations retain their MIT notice.

Copies of these skills or excerpts must retain the applicable license texts,
copyright notices and attribution, including those for adapted material.

## Bundled terminal themes

The iTerm2-Color-Schemes collection supplies Neptune's offline terminal palettes.
[Provenance and conversion notes](assets/themes/README.md), the upstream
[MIT license](assets/themes/LICENSE), and complete [author credits](assets/themes/CREDITS.md)
are retained in `assets/themes/` and copied into release archives.

## Agent marks

The marks of Claude Code, Codex, OpenCode and pi shown on terminal tabs are
trademarks of their respective owners, used only to identify the agent a
terminal runs. [Their sources](assets/icons/providers/README.md) are recorded
with the artwork. Neptune's license does not extend to them.

## Bundled fonts and dependencies

Geist and JetBrains Mono retain their SIL Open Font License 1.1 notices in
[`assets/fonts/`](assets/fonts/README.md).

Cargo dependencies retain their respective licenses. The release workflow
collects their license files and notices into `THIRD-PARTY-NOTICES.txt` in binary
archives. The development skills above are distributed with the source
repository.
