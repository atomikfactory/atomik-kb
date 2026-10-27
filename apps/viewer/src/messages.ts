// The viewer's own words (M13 slice 4): the states around the page, in English, French and Spanish, in the language the
// browser prefers (`navigator.languages`; English when none of the three is in the list). The page's own strings (the
// editor's, "Untitled") are `@atomik-kb/ui`'s and follow the browser the same way. Core error messages stay English.

/** The languages of the viewer. */
export type ViewerLanguage = "en" | "fr" | "es";

const en = {
  "title.fallback": "Shared page",
  "loading": "Opening the page...",
  "readOnly": "Shared with you to read. You cannot change this page.",
  "gone.title": "This link no longer works",
  "gone.body": "The page is no longer available: the link was turned off or has expired, or it never existed. Ask the person who shared it for a new link.",
  "invalid.title": "This link does not open",
  "invalid.body": "The key in the link is not right, or part of it was lost. Open the link exactly as you were given it.",
  "nokey.title": "This link has no key",
  "nokey.body": "The address must end with the key that comes after the # sign. Open the whole link you were given.",
  "failed.title": "The page could not be loaded",
  "failed.body": "The server did not answer. Check your connection and try again; the page will also try again by itself.",
  "retry": "Try again",
  "offline": "You are offline, or the server cannot be reached. This is the version loaded earlier; it will refresh when the server is back.",
} as const;

/** A key of the viewer's strings. */
export type MessageKey = keyof typeof en;

const fr: Readonly<Record<MessageKey, string>> = {
  "title.fallback": "Page partagée",
  "loading": "Ouverture de la page...",
  "readOnly": "Cette page est partagée en lecture seule. Vous ne pouvez pas la modifier.",
  "gone.title": "Ce lien ne fonctionne plus",
  "gone.body": "La page n'est plus disponible : le lien a été désactivé ou a expiré, ou il n'a jamais existé. Demandez un nouveau lien à la personne qui l'a partagée.",
  "invalid.title": "Ce lien ne s'ouvre pas",
  "invalid.body": "La clé du lien n'est pas correcte, ou une partie a été perdue. Ouvrez le lien exactement tel qu'on vous l'a donné.",
  "nokey.title": "Ce lien n'a pas de clé",
  "nokey.body": "L'adresse doit se terminer par la clé qui suit le signe #. Ouvrez le lien complet qu'on vous a donné.",
  "failed.title": "La page n'a pas pu être chargée",
  "failed.body": "Le serveur n'a pas répondu. Vérifiez votre connexion et réessayez ; la page réessaiera aussi d'elle-même.",
  "retry": "Réessayer",
  "offline":
    "Vous êtes hors ligne, ou le serveur est injoignable. Voici la version chargée plus tôt ; elle se mettra à jour au retour du serveur.",
};

const es: Readonly<Record<MessageKey, string>> = {
  "title.fallback": "Página compartida",
  "loading": "Abriendo la página...",
  "readOnly": "Esta página se comparte solo para leer. No puedes modificarla.",
  "gone.title": "Este enlace ya no funciona",
  "gone.body": "La página ya no está disponible: el enlace se desactivó o caducó, o nunca existió. Pide un enlace nuevo a la persona que la compartió.",
  "invalid.title": "Este enlace no se abre",
  "invalid.body": "La clave del enlace no es correcta o se perdió una parte. Abre el enlace exactamente como te lo dieron.",
  "nokey.title": "Este enlace no tiene clave",
  "nokey.body": "La dirección debe terminar con la clave que va después del signo #. Abre el enlace completo que te dieron.",
  "failed.title": "No se pudo cargar la página",
  "failed.body": "El servidor no respondió. Comprueba tu conexión e inténtalo de nuevo; la página también lo intentará sola.",
  "retry": "Reintentar",
  "offline":
    "Estás sin conexión, o no se puede llegar al servidor. Esta es la versión cargada antes; se actualizará cuando el servidor vuelva.",
};

const dictionaries: Readonly<Record<ViewerLanguage, Readonly<Record<MessageKey, string>>>> = { en, fr, es };

/** The language of the first tag of `preferred` that the viewer has (`fr-CA` is French), else English. */
export function pickLanguage(preferred: readonly string[]): ViewerLanguage {
  for (const tag of preferred) {
    const primary = tag.split(/[-_]/u)[0]?.toLowerCase();
    if (primary === "en" || primary === "fr" || primary === "es") {
      return primary;
    }
  }
  return "en";
}

/** The browser's languages, most preferred first. */
export function browserLanguages(): readonly string[] {
  const listed: readonly string[] = navigator.languages;
  return listed.length > 0 ? listed : navigator.language === "" ? [] : [navigator.language];
}

/** The text of `key` in `language`. */
export function message(language: ViewerLanguage, key: MessageKey): string {
  return dictionaries[language][key];
}
