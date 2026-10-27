// Starter: reading notes. The one idea to remember, key ideas, quotes, the reader's own thoughts and what to do next.

import type { StarterSource } from "./source";

export const READING_NOTES: StarterSource = {
  id: "reading-notes",
  icon: "📚",
  title: {
    en: "Reading notes",
    fr: "Notes de lecture",
    es: "Notas de lectura",
  },
  description: {
    en: "Key ideas, quotes and your own thoughts on a book or an article.",
    fr: "Idées clés, citations et réflexions personnelles sur un livre ou un article.",
    es: "Ideas clave, citas y tus propias reflexiones sobre un libro o un artículo.",
  },
  markdown: {
    en: `---
title: "Reading notes"
icon: "📚"
Status: "Reading"
Started: "{{today}}"
---

> 💡 The one idea I want to remember

## About

Title, author, and why I picked it up.

## Key ideas

- Idea
- Idea

## Quotes

> A passage worth keeping (with the page)

## My thoughts

What I agree with, what I question, and how it connects to other things I know.

## What to do next

- [ ] Something to try or apply
- [ ] Something to read next
`,
    fr: `---
title: "Notes de lecture"
icon: "📚"
Statut: "En cours"
Début: "{{today}}"
---

> 💡 L'idée que je veux retenir

## À propos

Titre, auteur, et pourquoi je l'ai choisi.

## Idées clés

- Idée
- Idée

## Citations

> Un passage à garder (avec la page)

## Ma réflexion

Ce que j'approuve, ce que je remets en question, et le lien avec ce que je connais déjà.

## Pour la suite

- [ ] Quelque chose à essayer ou à appliquer
- [ ] Une prochaine lecture
`,
    es: `---
title: "Notas de lectura"
icon: "📚"
Estado: "Leyendo"
Inicio: "{{today}}"
---

> 💡 La idea que quiero recordar

## Sobre la obra

Título, autor y por qué lo elegí.

## Ideas clave

- Idea
- Idea

## Citas

> Un pasaje que vale la pena guardar (con la página)

## Mis reflexiones

Con qué estoy de acuerdo, qué pongo en duda y cómo se conecta con lo que ya sé.

## Próximos pasos

- [ ] Algo para probar o aplicar
- [ ] Una próxima lectura
`,
  },
};
