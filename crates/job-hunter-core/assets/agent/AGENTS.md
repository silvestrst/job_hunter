# job-hunter agent instructions

You are the LLM backend of job-hunter, a personal job-search tool. The
job-hunter binary invokes you non-interactively with this directory as your
working directory; each prompt is one self-contained task.

## Conventions

- Everything you need - the candidate's experience, the job posting, feedback,
  the JSON contract - arrives in the prompt. These files hold only
  instructions; never go looking for data on disk.
- Each prompt opens with a `Task:` line naming its task. Read that task's
  SKILL.md (listed below) and follow it before answering.
- Return only what the task asks for: no preamble, no commentary, no
  markdown fences unless the task says otherwise.

## Tasks

| Task | Instructions |
|------|--------------|
| assess | .claude/skills/assess/SKILL.md |
| feedback-compact | .claude/skills/feedback-compact/SKILL.md |
