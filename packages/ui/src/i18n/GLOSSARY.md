# Translation glossary

The terms of the UI, so that every language says the same thing the same way (ADR-0040 item 13). English is the
source (`en.json`); French is `fr.json`; Spanish is `es.json`. Keep this table in step when a term is added.

## Rules for every language

- **Keys.** Every key of `en.json` exists in every language file and keeps the same `{placeholders}`, no more and no
  fewer (`i18n.test.ts` checks both). New strings go into `en.json`, `fr.json` and `es.json` in the same change.
- **Register.** The UI register, sentence case like the English, short sentences.
  - **French:** formal "vous".
  - **Spanish (Spain):** "tú", never "usted" (so "tu", "tus", never "su"), the way most modern apps write. Buttons,
    menu items and titles that name an action use the infinitive (`Guardar`, `Eliminar página`); a sentence that
    speaks to the person uses the "tú" imperative (`Escribe tu frase de contraseña`, `Elige un modelo`). A message
    that reports what happened has no subject where it can (`No se pudo guardar el archivo`). Words are the ones of
    Spain: `ordenador`, `introducir` (a value), `escribir` (a text), `añadir`, `archivo`, `contraseña`.
- **Not translated.** Product names (atomik-kb, Ollama, MCP, Markdown, Notion, AppFlowy, Excalidraw, LaTeX, Mermaid,
  PDF, HTML, CSV, URL), file extensions, the keyboard keys (Ctrl, Alt, Super, `Ctrl+K`) and what the core says (core
  error messages stay English).
- **Plurals.** A count is a pair `x.one` / `x.other` (`tp()` picks it with `Intl.PluralRules` of the language). `.one`
  is not always 1 (in French it is 0 and 1), so write the number as `{count}`, never as "1". A language that needs
  another category (`.many`, `.few`) adds that key beside the two. Spanish has `.many` only for multiples of a million,
  which the UI never counts (and the parity test allows no key that `en.json` lacks), so it lets `tp()` fall back
  to `.other`.
- **Punctuation.** French writes a no-break space (U+00A0) before `:` `;` `?` `!` and inside « guillemets ». Spanish
  writes « guillemets » without inner spaces for a quoted name, and opens a question and an exclamation with ¿ and ¡.
  Both use the typographic apostrophe ’ and the one-character ellipsis …
- **Shared spaces** (`sharing.*`). A space that is no longer updated (a closed one, or one the person was removed from) is a
  "read-only copy", never an "archive": in Spanish `archivo` is a file, so it is a `copia`. The four roles are nouns in the
  masculine in French (`Propriétaire`, `Éditeur`, `Commentateur`, `Lecteur`), and a sentence that names one says "en tant que
  Lecteur" (`como Lector` in Spanish). Safety numbers, codes and the six digits are never translated or reformatted.
- **Native labels** (tray, menu bar, dialogs, file types: `tray.*`, `menu.*`, `shellDialog.*`, `fileFilter.*`). The
  shell shows them, and it refuses what does not fit: a label is 1 to 64 characters, a dialog text up to 400, no
  control characters. A menu label has one `&` before the letter that opens the item with Alt (`&Fichier`,
  `&Archivo`), and the mnemonics of one menu must all differ. The consent texts must keep the placeholders that say
  what is agreed to (`{host}`, `{client}`, `{spaces}`, `{read}`, `{write}`, `{stored}`, and for the download of a speech
  model `{size}` and `{host}`; `{model}` is optional): without them the shell refuses the whole message.
  `shellDialog.imagesOne` and `shellDialog.hostsOne` are the singular forms, the shell picks them for exactly one.

## Terms

| English | French | Spanish |
| --- | --- | --- |
| page | page | página |
| sub-page | sous-page | subpágina |
| space | espace | espacio |
| workspace | espace de travail | espacio de trabajo |
| sidebar | barre latérale | barra lateral |
| tab | onglet | pestaña |
| trash | corbeille | papelera |
| favourites | favoris | favoritos |
| recent pages | pages récentes | páginas recientes |
| settings | paramètres | ajustes |
| sync | synchronisation | sincronización |
| sync server | serveur de synchronisation | servidor de sincronización |
| device | appareil | dispositivo |
| pairing (a device) | association | emparejamiento |
| recovery kit | kit de récupération | kit de recuperación |
| re-issue (the kit) | régénérer | regenerar |
| passphrase | phrase secrète | frase de contraseña |
| app lock | verrouillage de l’application | bloqueo de la aplicación |
| lock / unlock | verrouiller / déverrouiller | bloquear / desbloquear |
| backup | sauvegarde | copia de seguridad |
| restore | restaurer | restaurar |
| import / export | importer / exporter | importar / exportar |
| property | propriété | propiedad |
| tag | étiquette | etiqueta |
| relation | relation | relación |
| rollup | cumul | acumulado |
| formula | formule | fórmula |
| view | vue | vista |
| table / board / list | tableau / tableau Kanban / liste | tabla / tablero / lista |
| block | bloc | bloque |
| toggle (block) | bloc dépliable | desplegable |
| toggle heading | titre dépliable | encabezado desplegable |
| callout | encadré | destacado |
| divider | séparateur | separador |
| heading | titre | encabezado |
| quote | citation | cita |
| to-do list | liste de tâches | lista de tareas |
| bulleted / numbered list | liste à puces / liste numérotée | lista con viñetas / lista numerada |
| checklist | liste de contrôle | lista de verificación |
| attachment | pièce jointe | archivo adjunto |
| file | fichier | archivo |
| cover | couverture | portada |
| icon | icône | icono |
| whiteboard | tableau blanc | pizarra |
| gallery | galerie | galería |
| caption | légende | leyenda |
| mention | mention | mención |
| reminder | rappel | recordatorio |
| breadcrumb | fil d’Ariane | ruta de navegación |
| table of contents | table des matières | tabla de contenido |
| page info | informations sur la page | información de la página |
| search | rechercher / recherche | buscar / búsqueda |
| find in page | rechercher dans la page | buscar en la página |
| highlight | surlignage | resaltado |
| accent colour | couleur d’accentuation | color de énfasis |
| theme (light / dark / match system) | thème (clair / sombre / suivre le système) | tema (claro / oscuro / seguir el sistema) |
| language | langue | idioma |
| shortcut | raccourci | atajo |
| tray (notification area) | zone de notification | bandeja del sistema |
| menu bar | barre de menus | barra de menús |
| quick capture | capture rapide | captura rápida |
| local AI | IA locale | IA local |
| model (AI) | modèle | modelo |
| semantic index | index sémantique | índice semántico |
| transcript / transcribe | transcription / transcrire | transcripción / transcribir |
| client (MCP) | client | cliente |
| key / token | clé / jeton | clave / token |
| permissions | autorisations | permisos |
| delete (permanently) | supprimer (définitivement) | eliminar (definitivamente) |
| remove (from a list, a page, an account) | retirer | quitar |
| dismiss (a message) | ignorer | descartar |
| discard (an AI answer) | abandonner | descartar |
| try again / retry | réessayer | reintentar |
| untitled | sans titre | sin título |
| move up / down | monter / descendre | subir / bajar |
| sort (a list, pages) / A to Z / date created | trier / de A à Z / date de création | ordenar / de A a Z / fecha de creación |
| rename | renommer | renombrar |
| duplicate | dupliquer | duplicar |
| copy / paste / cut | copier / coller / couper | copiar / pegar / cortar |
| undo / redo | annuler / rétablir | deshacer / rehacer |
| cancel / close / done | annuler / fermer / terminé | cancelar / cerrar / listo |
| add / create / save / open | ajouter / créer / enregistrer / ouvrir | añadir / crear / guardar / abrir |
| choose / pick | choisir | elegir |
| database (the views of one source in a block: list, table, board, calendar, gallery) | base de données | base de datos |
| view tab (a view of a database, in the strip above it) | onglet de vue | pestaña de vista |
| calendar (a layout) | calendrier | calendario |
| gallery (a layout; the photo gallery block is `galerie photo` / `galería de fotos`) | galerie | galería |
| card (a page in the board or the gallery) | carte | tarjeta |
| peek (a page shown beside or over the one that holds the database) | aperçu | vista previa |
| side peek / center peek | aperçu latéral / aperçu centré | vista previa lateral / vista previa central |
| filter group (filters combined with all / any) | groupe de filtres | grupo de filtros |
| match all / any filter | correspondre à tous les filtres / à au moins un filtre | cumplir todos los filtros / alguno de los filtros |
| sort key (a view sorts by up to five) | tri (« Ajouter un tri ») | criterio de orden |
| relative date (Today, Yesterday, Tomorrow) / period (This week, Past days) | date relative (Aujourd’hui, Hier, Demain) / période (Cette semaine, Jours passés) | fecha relativa (Hoy, Ayer, Mañana) / periodo (Esta semana, Días anteriores) |
| no date (the calendar tray of pages without a date) | sans date | sin fecha |
| dashboard (a layout: a grid of widgets) | tableau de bord | panel |
| widget (a number, a chart or a view of a dashboard; kept as `widget` in both languages) | widget | widget |
| enter (a value) / type (a text) | saisir / taper | introducir / escribir |
| computer | ordinateur | ordenador |
| show / hide | afficher / masquer | mostrar / ocultar |
| expand / collapse | développer / réduire | expandir / contraer |
| indent / outdent | mettre en retrait / retirer le retrait | aumentar / reducir sangría |
| unwrap (columns) | dissocier | desagrupar |
| shared space | espace partagé | espacio compartido |
| member | membre | miembro |
| role | rôle | rol |
| owner / editor / commenter / viewer | propriétaire / éditeur / commentateur / lecteur | propietario / editor / comentarista / lector |
| invitation | invitation | invitación |
| invitation link / link for a browser | lien d’invitation / lien pour un navigateur | enlace de invitación / enlace para un navegador |
| join (a space) | rejoindre | unirse a |
| leave / close (a space) | quitter / fermer | salir de / cerrar |
| safety number | numéro de sécurité | número de seguridad |
| verified / not verified / mark verified | vérifié / non vérifié / marquer comme vérifié | verificado / sin verificar / marcar como verificado |
| read-only copy (a space that is no longer updated) | copie en lecture seule | copia de solo lectura |
| history (of a space) | historique | historial |
| view link | lien de consultation | enlace de solo lectura |
| comment (the noun, and the verb) | commentaire / commenter | comentario / comentar |
| thread (of comments) | fil de discussion | hilo |
| resolve / reopen (a thread) | résoudre / rouvrir | resolver / reabrir |
| notification / inbox | notification / boîte de réception | notificación / bandeja de entrada |
| person (the property, a mention of someone) | personne | persona |
| former member (someone who left or was removed) | ancien membre | antiguo miembro |
| presence (who is on a page, in a space) / idle | présence / inactif | presencia / inactivo |
| avatar (the circle of initials of a person who is present) | pastille | avatar |
| dictation (speech typed at the cursor, `/microphone`) | dictée | dictado |
| microphone | microphone | micrófono |
| speech engine (Accurate, Light, Ollama) / speech model | moteur vocal (Précis, Léger, Ollama) / modèle vocal | motor de voz (Preciso, Ligero, Ollama) / modelo de voz |
| download (a model) / use model files (from a folder) | télécharger / utiliser des fichiers de modèle | descargar / usar archivos del modelo |
| fingerprint (the checked hash of a downloaded file) | empreinte | huella |
| recognise (speech) | reconnaître | reconocer |
| template (a page marked as a template, ADR-0056) / starter (a template that ships with the app) | modèle / modèle de départ | plantilla / plantilla inicial |
| turn into template / stop using as template | transformer en modèle / ne plus utiliser comme modèle | convertir en plantilla / dejar de usar como plantilla |
| placeholder (a value filled in when a page is made from a template: Today, Now, Me) / filled in | remplissage automatique / rempli automatiquement | relleno automático / relleno automático |
| upload (a backup to Google Drive) | téléverser | subir |
| connect / disconnect (Google Drive) / reconnect | connecter / déconnecter / reconnecter | conectar / desconectar / reconectar |
| sign in (to Google, in the browser) | se connecter | iniciar sesión |
| Google client (the OAuth client the app signs in with; id and secret) | client Google (ID client, code secret du client) | cliente de Google (ID de cliente, secreto de cliente) |
