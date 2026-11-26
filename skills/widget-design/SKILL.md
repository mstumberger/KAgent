---
name: widget-design
description: "Draws a KAgent widget: the HTML panel shown inside a chat answer (a gauge, bars, a table, a sparkline). Load it before the first widget_show call, or when the owner asks to see numbers as a picture."
when_to_use: before the first widget_show call of a session; when a panel came out unreadable, too tall or cut off at the side.
---
A widget is a small static HTML panel inside the chat answer, shown in its own frame.
It holds the numbers of that moment, written into it: nothing in it loads, asks for
or changes anything on the machine.

## When to make one

When a picture says more than the numbers: one score, a few values to compare, a
short trend, a ranked list. A plain table the owner asked for stays Markdown. Say in
words what the panel shows; do not repeat its numbers in the text.

## Order of work

1. Read the data first (`doctor`, `disk_info`, `process_list`, `traffic_top`).
2. Choose the one idea: a gauge for one score, bars for parts of one whole, a table
   for ranked rows, a sparkline for a trend.
3. Lay out for 300 px wide: the value or the name on top, the rest below.
4. Fill that layout with the numbers, then call `widget_show` once, with the final data.
5. Put the marker line it answers on its own line, where the panel belongs.

## Rules

- Layout first, then content. Fill the width: nothing fixed and wide, no scroll bar.
  What does not fit at 300 px stacks. One gauge is about 160 px tall.
- Colours only through the theme variables, so the panel follows the theme:
  `--k-night` (page), `--k-glass` (panel), `--k-ink` (text), `--k-dim` (secondary text,
  a track), `--k-ice` (the accent), `--k-ok` and `--k-bad` (good and bad),
  `--k-signal` (needs a look). No other colour.
- Under 120 lines; CSS and JS inline, in a `<style>` and a `<script>`.
- No external fonts, scripts, images or links. No forms. No `fetch`.
- Nothing loops forever and nothing animates without end: a script may answer a
  click and then stop.
- No `html`, `head` or `body` tags, and no explanation inside the panel: labels,
  values and a legend only. Text of 12 px or more, in `--k-ink`.

## A gauge looks like

```html
<style>
.g { font: 14px system-ui; color: var(--k-ink); background: var(--k-glass); padding: 12px; border-radius: 10px; }
.bar { height: 10px; background: var(--k-dim); border-radius: 5px; }
.bar i { display: block; height: 100%; width: 85%; background: var(--k-ok); border-radius: 5px; }
</style>
<div class="g">Doctor score <b>85</b> of 100
<div class="bar"><i></i></div></div>
```

Bars: the same `div` per value, each with its own `width`. A table: `<table>` with
`th` in `--k-dim`. A sparkline: one inline `<svg>` whose `<polyline>` is `--k-ice`.
