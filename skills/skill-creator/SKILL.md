---
name: skill-creator
description: Guides KAgent through writing a new skill, or improving one, whenever the owner asks to remember how something was done, to save a fix as a skill, or to turn a diagnosis that keeps coming back into a playbook. Load it before any skill_save call.
when_to_use: after a non-trivial fix that is likely to recur, when the same diagnosis was done twice, or when the owner says "remember this" or "make a skill".
---
A skill is a short `SKILL.md` that KAgent loads with `load_skill` when a request
matches its description. It holds what a model cannot know on its own: the order of
steps that worked here, which tools to use, what the numbers mean, and what the owner
wants to see at the end.

## What goes in

- `name`: lowercase words joined by hyphens, at most 64 characters, named after what
  it does (`fix-bluetooth`, `router-unblock`, `release-checklist`).
- `description`: the trigger, so the most important line. Write it in the third person,
  as the words the owner would say ("Fixes Bluetooth that pairs but stays silent. Use
  when ..."). Say what it is for and when to use it: this is all KAgent sees before
  it decides to load the skill.
- `when_to_use`: only what the description cannot hold (a symptom that sounds like
  something else, a condition, a time).
- `body`: numbered steps that name the tools (`process_list`, `journal`, `package`),
  what to look for in each answer, when to stop and ask, and what is never done
  without the owner. End it with "A good answer looks like:" and one example.

## How to write one

1. Gather what actually happened: the tools that ran, the numbers they returned, the
   commands that worked, and what was tried and failed.
2. Keep what is not obvious and drop what any model knows. Stay under 60 lines.
3. Name tools, never machine-specific absolute paths or secrets: say "the router" or
   "the project folder" and let memory hold addresses, passwords and keys. A skill is
   read again on every later use.
4. Save it with `skill_save`. If a bundled skill has the same name, say that the new
   one hides it. Show the description in the reply, so the owner can judge the trigger.
5. Check it as a first-time user would: `load_skill` the new name and read it.

## When to offer a skill

Offer one when the owner asks to remember how something was done, when a non-trivial
fix is likely to recur, or when the same diagnosis was done twice. Save only what the
owner asks for or agrees to. One question is enough: "Save this as a skill?"

## A good answer looks like

"Saved `fix-bluetooth`: it restarts bluetooth, removes the stale pairing and pairs
the headphones again, and it triggers on 'no sound in my headphones'. Nothing else
on the machine was changed."
