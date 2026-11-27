# KAgent

A local agent that diagnoses and changes this machine, controls the desktop by voice
or text, and watches for problems and intruders. The owner talks to it in chat and by
voice.

## Language

**Owner**:
The one person KAgent works for and takes instructions from.
_Avoid_: user (in specs), admin

**Brain**:
The chat model: the model that answers chat turns and, in routed and auto voice, does
the thinking behind the voice.
_Avoid_: main model, thinking model

**Watcher**:
A standing rule that reads the journal and system state and raises an alert when it
matches.
_Avoid_: monitor, screen watcher

**Screen watch**:
Watching the screen on the owner's request, with a goal and a time limit, and speaking
up when the goal is met or something relevant appears.
_Avoid_: watcher, observe, look-out

**Recommended model**:
A local model KAgent suggests for a job (chat, vision, embedding) because it fits this
machine's graphics memory, and can download on request.
_Avoid_: suggested model, default model

**Morning report**:
The owner's day at a glance, delivered at a set time: today's events, when to leave
for the first one, tomorrow's first event, and KAgent's own tasks for the day.
_Avoid_: daily digest, briefing, agenda

## Work

**Worker**:
One agent doing one step of a run: a subagent of KAgent or a coding agent.
_Avoid_: bot, child

**Role**:
What a worker is for in a workflow: planner, implementer or reviewer.

**Workflow**:
A saved recipe of roles: which run in which order, with which worker and model, and
where the owner must approve.
_Avoid_: pipeline, automation

**Run**:
One execution of a workflow on one job in one project.
_Avoid_: job, session

**Task**:
One unit of work inside a run, produced by the planner, with what "done" means.
_Avoid_: scheduled task (that is a reminder or an automation), todo

**Round**:
One pass of implementing a task and reviewing it.

**Board**:
The view that shows runs, their tasks and their workers.

## Voice

**Live voice**:
Voice through one speech-to-speech model that listens, thinks and speaks.

**Pipeline voice**:
Voice through three parts: speech-to-text, the Brain, and text-to-speech, each from
its own provider.

**Voice provider**:
A service or local engine that does speech-to-speech, speech-to-text or text-to-speech
for KAgent.
