// Starter: project brief. The one-sentence summary, goals, scope, success measures, a short timeline and the risks.

import { NBSP } from "./source";
import type { StarterSource } from "./source";

export const PROJECT_BRIEF: StarterSource = {
  id: "project-brief",
  icon: "🎯",
  title: {
    en: "Project brief",
    fr: "Fiche projet",
    es: "Resumen del proyecto",
  },
  description: {
    en: "Goals, scope, timeline and risks on one page.",
    fr: "Objectifs, périmètre, calendrier et risques sur une page.",
    es: "Objetivos, alcance, calendario y riesgos en una página.",
  },
  markdown: {
    en: `---
title: "Project brief"
icon: "🎯"
Status: "Draft"
---

> 💡 In one sentence: what are we building, for whom, and why does it matter?

## Background

Why now? What problem or opportunity led to this project?

## Goals

- Goal
- Goal

## Out of scope

- What we are not doing

## Success measures

- How we will know it worked

## Timeline

Kick-off: <time datetime="{{today}}">{{today}}</time>

- [ ] First milestone
- [ ] Second milestone
- [ ] Launch

## People

Owner, contributors and who needs to be kept informed.

<details>
<summary>Risks and open questions</summary>

- Risk or question, and who will look into it

</details>
`,
    fr: `---
title: "Fiche projet"
icon: "🎯"
Statut: "Brouillon"
---

> 💡 En une phrase${NBSP}: que construisons-nous, pour qui, et pourquoi est-ce important${NBSP}?

## Contexte

Pourquoi maintenant${NBSP}? Quel problème ou quelle opportunité est à l'origine du projet${NBSP}?

## Objectifs

- Objectif
- Objectif

## Hors périmètre

- Ce que nous ne ferons pas

## Critères de réussite

- Comment saurons-nous que ça a fonctionné

## Calendrier

Lancement du projet${NBSP}: <time datetime="{{today}}">{{today}}</time>

- [ ] Première étape
- [ ] Deuxième étape
- [ ] Livraison

## Équipe

Responsable, contributeurs et personnes à tenir informées.

<details>
<summary>Risques et questions ouvertes</summary>

- Risque ou question, et qui s'en occupe

</details>
`,
    es: `---
title: "Resumen del proyecto"
icon: "🎯"
Estado: "Borrador"
---

> 💡 En una frase: ¿qué estamos construyendo, para quién y por qué es importante?

## Contexto

¿Por qué ahora? ¿Qué problema u oportunidad dio origen al proyecto?

## Objetivos

- Objetivo
- Objetivo

## Fuera del alcance

- Lo que no vamos a hacer

## Cómo medimos el éxito

- Cómo sabremos que funcionó

## Calendario

Inicio: <time datetime="{{today}}">{{today}}</time>

- [ ] Primer hito
- [ ] Segundo hito
- [ ] Lanzamiento

## Equipo

Responsable, colaboradores y a quién hay que mantener informado.

<details>
<summary>Riesgos y preguntas abiertas</summary>

- Riesgo o pregunta, y quién lo investigará

</details>
`,
  },
};
