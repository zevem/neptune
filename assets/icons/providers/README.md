# Agent marks

The marks a terminal tab shows for the CLI agent it runs. Each SVG is the
artwork as [T3 Code](https://github.com/pingdotgg/t3code) carries it in
`apps/web/src/components/Icons.tsx` at revision
`115640ae49924c9e4e6215bd8d911445ad932bff`, filled white. `opencode.svg` keeps
the two tones of its mark as two strengths of one ink.

| File | Agent | T3 Code component |
| --- | --- | --- |
| `claude.svg` | Claude Code | `ClaudeAI` |
| `codex.svg` | Codex | `OpenAI` |
| `opencode.svg` | OpenCode | `OpenCodeIcon` |
| `pi.svg` | pi | `PiAgentIcon` |

Gemini CLI and Oh My Pi have no mark here; their tabs show none.

Neptune embeds the 64-pixel PNG masks and tints them when drawing. Regenerate
them from the repository root after changing an SVG (needs PyGObject with
librsvg, and pycairo):

```sh
python3 scripts/export-provider-icons.py
```

The marks are trademarks of their owners and identify those agents only. They
are not covered by Neptune's license.
