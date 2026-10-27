// Starter: weekly review. Look back at the week (wins, what went wrong, lessons), plan the next one, prompts in a toggle.

import { NBSP } from "./source";
import type { StarterSource } from "./source";

export const WEEKLY_REVIEW: StarterSource = {
  id: "weekly-review",
  icon: "📅",
  title: {
    en: "Weekly review",
    fr: "Bilan hebdomadaire",
    es: "Revisión semanal",
  },
  description: {
    en: "Look back at the week and plan the next one.",
    fr: "Faire le point sur la semaine et préparer la suivante.",
    es: "Repasa la semana y planifica la siguiente.",
  },
  markdown: {
    en: `---
title: "Weekly review {{date}}"
icon: "📅"
Date: "{{today}}"
---

Week ending <time datetime="{{today}}">{{today}}</time>.

## Wins

- What went well this week

## What did not go to plan

- What slipped, and why

## Lessons

What I take away from this week.

## Next week

- [ ] Priority
- [ ] Priority
- [ ] Priority

<details>
<summary>Questions to ask yourself</summary>

- Did my time go to what matters most?
- What gave me energy, and what drained it?
- What should I stop, start or keep doing?

</details>
`,
    fr: `---
title: "Bilan de la semaine {{date}}"
icon: "📅"
Date: "{{today}}"
---

Semaine se terminant le <time datetime="{{today}}">{{today}}</time>.

## Réussites

- Ce qui a bien marché cette semaine

## Ce qui ne s'est pas passé comme prévu

- Ce qui a pris du retard, et pourquoi

## Enseignements

Ce que je retiens de cette semaine.

## Semaine prochaine

- [ ] Priorité
- [ ] Priorité
- [ ] Priorité

<details>
<summary>Questions à se poser</summary>

- Mon temps est-il allé à ce qui compte le plus${NBSP}?
- Qu'est-ce qui m'a donné de l'énergie, et qu'est-ce qui m'en a pris${NBSP}?
- Que devrais-je arrêter, commencer ou continuer${NBSP}?

</details>
`,
    es: `---
title: "Revisión semanal {{date}}"
icon: "📅"
Fecha: "{{today}}"
---

Semana que termina el <time datetime="{{today}}">{{today}}</time>.

## Logros

- Lo que salió bien esta semana

## Lo que no salió según lo previsto

- Lo que se retrasó, y por qué

## Aprendizajes

Lo que me llevo de esta semana.

## Próxima semana

- [ ] Prioridad
- [ ] Prioridad
- [ ] Prioridad

<details>
<summary>Preguntas para reflexionar</summary>

- ¿Mi tiempo fue a lo que más importa?
- ¿Qué me dio energía y qué me la quitó?
- ¿Qué debería dejar, empezar o seguir haciendo?

</details>
`,
  },
};
