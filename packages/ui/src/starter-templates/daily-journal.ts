// Starter: daily journal. A page a day: today's date, priorities, gratitude, notes and a reflection to open in the evening.

import { NBSP } from "./source";
import type { StarterSource } from "./source";

export const DAILY_JOURNAL: StarterSource = {
  id: "daily-journal",
  icon: "📔",
  title: {
    en: "Daily journal",
    fr: "Journal quotidien",
    es: "Diario",
  },
  description: {
    en: "A page a day: priorities, gratitude, notes and a short reflection.",
    fr: `Une page par jour${NBSP}: priorités, gratitude, notes et un court bilan.`,
    es: "Una página al día: prioridades, gratitud, notas y una breve reflexión.",
  },
  markdown: {
    en: `---
title: "Journal {{date}}"
icon: "📔"
Date: "{{today}}"
---

Today is <time datetime="{{today}}">{{today}}</time>.

## Top priorities

- [ ] The one thing that matters most today
- [ ] Second priority
- [ ] Third priority

## Gratitude

- Something I am thankful for

## Notes

What happened, what I noticed, what is on my mind.

<details>
<summary>Evening reflection</summary>

- What went well?
- What would I do differently?
- What am I looking forward to tomorrow?

</details>
`,
    fr: `---
title: "Journal {{date}}"
icon: "📔"
Date: "{{today}}"
---

Aujourd'hui, nous sommes le <time datetime="{{today}}">{{today}}</time>.

## Priorités du jour

- [ ] Ce qui compte le plus aujourd'hui
- [ ] Deuxième priorité
- [ ] Troisième priorité

## Gratitude

- Une chose qui m'a fait plaisir aujourd'hui

## Notes

Ce qui s'est passé, ce que j'ai remarqué, ce qui me préoccupe.

<details>
<summary>Bilan du soir</summary>

- Qu'est-ce qui s'est bien passé${NBSP}?
- Que ferais-je autrement${NBSP}?
- Qu'est-ce que j'attends avec impatience demain${NBSP}?

</details>
`,
    es: `---
title: "Diario {{date}}"
icon: "📔"
Fecha: "{{today}}"
---

Hoy es <time datetime="{{today}}">{{today}}</time>.

## Prioridades del día

- [ ] Lo más importante de hoy
- [ ] Segunda prioridad
- [ ] Tercera prioridad

## Gratitud

- Algo que agradezco

## Notas

Lo que pasó, lo que noté, lo que tengo en la cabeza.

<details>
<summary>Reflexión de la noche</summary>

- ¿Qué salió bien?
- ¿Qué haría de otra manera?
- ¿Qué espero con ilusión para mañana?

</details>
`,
  },
};
