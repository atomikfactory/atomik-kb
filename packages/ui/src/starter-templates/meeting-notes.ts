// Starter: meeting notes. Attendees, agenda, notes, decisions, action items and a toggle for what to keep for later.

import { NBSP } from "./source";
import type { StarterSource } from "./source";

export const MEETING_NOTES: StarterSource = {
  id: "meeting-notes",
  icon: "📝",
  title: {
    en: "Meeting notes",
    fr: "Notes de réunion",
    es: "Notas de reunión",
  },
  description: {
    en: "Attendees, agenda, decisions and action items.",
    fr: "Participants, ordre du jour, décisions et actions à mener.",
    es: "Asistentes, orden del día, decisiones y tareas.",
  },
  markdown: {
    en: `---
title: "Meeting notes {{date}}"
icon: "📝"
Date: "{{today}}"
---

## Attendees

Who is in the meeting?

## Agenda

1. First topic
2. Second topic

## Notes

What was said, and what was decided.

## Decisions

- Decision

## Action items

- [ ] Task, with who does it and by when
- [ ] Task

<details>
<summary>Parking lot</summary>

Ideas and questions for another time.

</details>
`,
    fr: `---
title: "Notes de réunion {{date}}"
icon: "📝"
Date: "{{today}}"
---

## Participants

Qui est présent${NBSP}?

## Ordre du jour

1. Premier sujet
2. Deuxième sujet

## Notes

Ce qui a été dit, et ce qui a été décidé.

## Décisions

- Décision

## Actions à mener

- [ ] Tâche, avec la personne responsable et l'échéance
- [ ] Tâche

<details>
<summary>À traiter plus tard</summary>

Idées et questions pour une autre fois.

</details>
`,
    es: `---
title: "Notas de reunión {{date}}"
icon: "📝"
Fecha: "{{today}}"
---

## Asistentes

¿Quién participa en la reunión?

## Orden del día

1. Primer tema
2. Segundo tema

## Notas

Lo que se dijo y lo que se decidió.

## Decisiones

- Decisión

## Tareas

- [ ] Tarea, con responsable y fecha límite
- [ ] Tarea

<details>
<summary>Para otro momento</summary>

Ideas y preguntas para tratar más adelante.

</details>
`,
  },
};
