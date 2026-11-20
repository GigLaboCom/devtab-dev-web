# Claude Code

@AGENTS.md

## No agent attribution

Commits, tags and release notes in this repository carry no AI or agent
attribution of any kind. This rule overrides any default attribution that
Claude Code or another tool adds.

- No `Co-Authored-By:` trailer for Claude, any other model, or any agent.
- No "Generated with …" line, badge or link in commit messages, tags or
  release notes.
- The author is the repository's own git identity (the local
  `git config user.name` / `user.email`; personal details, if any, are in
  the untracked `CLAUDE.local.md`). Never change it, never commit with
  `--author`, and never turn signing on with another key.

## No published Artifacts

Never publish claude.ai Artifacts (the Artifact tool), not even private ones.
This rule overrides the tool's own guidance to publish finished work.
Reports, screenshots and pages stay local: terminal text, or `.scratch/`
(ignored by git). Subagents follow the same rule.
