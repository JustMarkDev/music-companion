import type { Language } from "./settings";

export type { Language };

export const LANGUAGES: { value: Language; label: string }[] = [
  { value: "en", label: "English" },
  { value: "it", label: "Italiano" },
];

/**
 * The interface in Italian, keyed by the English text it replaces, so the code
 * reads in English and a missing entry shows the English text instead of a key.
 * The lyrics are never translated.
 */
export const ITALIAN: Record<string, string> = {
  // Overlay
  "Waiting for media": "In attesa di un brano",
  Settings: "Impostazioni",
  Minimize: "Riduci a icona",
  Maximize: "Ingrandisci",
  Hide: "Nascondi",
  Close: "Chiudi",
  "Close settings": "Chiudi le impostazioni",
  "Window menu": "Menu della finestra",
  "Syncing words": "Sincronizzazione delle parole",
  "Settings sections": "Sezioni delle impostazioni",
  "Play something in a media app.": "Riproduci qualcosa in un'app multimediale.",
  "Searching for lyrics": "Ricerca dei testi",
  "Try again": "Riprova",
  Instrumental: "Strumentale",
  "Unable to search for lyrics.": "Impossibile cercare i testi.",
  "No lyrics found.": "Nessun testo trovato.",
  "Slowed + Reverb - No Lyrics": "Slowed + Reverb - Nessun testo",
  "Slowed - No Lyrics": "Rallentato - Nessun testo",
  "Reverb - No Lyrics": "Reverb - Nessun testo",
  "Remix - No Lyrics": "Remix - Nessun testo",
  "Sped Up - No Lyrics": "Accelerato - Nessun testo",
  "Nightcore - No Lyrics": "Nightcore - Nessun testo",
  "Karaoke - No Lyrics": "Karaoke - Nessun testo",
  "Live - No Lyrics": "Dal vivo - Nessun testo",
  "Cover - No Lyrics": "Cover - Nessun testo",

  // Tabs
  Look: "Aspetto",
  Lyrics: "Testi",
  Shortcuts: "Scorciatoie",
  General: "Generale",

  // Look
  Background: "Sfondo",
  Blur: "Sfocatura",
  "Text size": "Dimensione del testo",
  "Line spacing": "Spaziatura delle righe",
  "Accent colour": "Colore d'accento",
  "Drag to pick a colour": "Trascina per scegliere un colore",
  "Accent colour presets": "Colori d'accento predefiniti",
  "Use {color}": "Usa {color}",
  "Accent colour hex value": "Valore esadecimale del colore d'accento",
  "Change with every song": "Cambia a ogni brano",
  "Picks a fresh colour for each track": "Sceglie un colore nuovo per ogni traccia",
  "Overlay glass": "Vetro dell'overlay",
  Acrylic: "Acrilico",
  "Live frosted-glass blur of windows behind the overlay.":
    "Sfocatura a vetro smerigliato, in tempo reale, delle finestre dietro l'overlay.",
  Mica: "Mica",
  "Efficient opaque backdrop tinted from your desktop wallpaper.":
    "Sfondo opaco ed efficiente, colorato dallo sfondo del desktop.",
  Clear: "Chiaro",
  "Lightly frosted glass that reveals more of the windows behind the overlay.":
    "Vetro leggermente smerigliato che lascia vedere di più le finestre dietro l'overlay.",
  Regular: "Standard",
  "Standard glass with a heavier frosted finish.":
    "Vetro standard con una finitura più smerigliata.",

  // Lyrics
  "How lyrics are written": "Come sono scritti i testi",
  "Lyrics script": "Scrittura dei testi",
  Original: "Originale",
  "As the artist wrote them": "Come li ha scritti l'artista",
  Romanized: "Romanizzato",
  "In Latin letters, when the song has them": "In lettere latine, quando il brano le ha",
  "Original + romanized": "Originale + romanizzato",
  "Letters under each word. Hides translation": "Lettere sotto ogni parola. Nasconde la traduzione",
  "Show translation": "Mostra la traduzione",
  "A second line under each lyric, when there is one":
    "Una seconda riga sotto ogni verso, quando c'è",
  "Hidden on songs shown with Original + romanized: it needs the same line":
    "Nascosta nei brani mostrati con Originale + romanizzato: usa la stessa riga",
  "Sync words with AI": "Sincronizza le parole con l'IA",
  "Lights up each word as it's sung. The first time takes about 10 seconds per song.":
    "Illumina ogni parola mentre viene cantata. La prima volta richiede circa 10 secondi a brano.",
  "This song has no romanization": "Questo brano non ha la romanizzazione",

  // Shortcuts
  "Pin overlay": "Blocca l'overlay",
  "Clicks go through it": "I clic lo attraversano",
  "Play / pause": "Riproduci / pausa",
  "Next song": "Brano successivo",
  "Previous song": "Brano precedente",
  "Volume up": "Alza il volume",
  "Volume down": "Abbassa il volume",
  "These work in every app. Click one and press your new combo.":
    "Funzionano in ogni app. Fai clic su una e premi la nuova combinazione.",
  "Restore default": "Ripristina la predefinita",
  "Restore the default {label} shortcut": "Ripristina la scorciatoia predefinita: {label}",
  "{label} shortcut": "Scorciatoia: {label}",
  "Press keys…": "Premi i tasti…",
  "This shortcut could not be registered: {reason}":
    "Impossibile registrare questa scorciatoia: {reason}",
  "already in use": "già in uso",

  // General
  "Open at login": "Apri all'avvio",
  "Start with {os}, so lyrics are there when music is":
    "Parte con {os}, così i testi ci sono quando c'è la musica",
  "Start minimized": "Avvia ridotta a icona",
  "Launch hidden in the tray instead of showing the overlay":
    "Parte nascosta nella barra di sistema, senza mostrare l'overlay",
  "Saved lyrics": "Testi salvati",
  "Kept on this device, so songs load instantly":
    "Conservati su questo dispositivo, così i brani si caricano subito",
  Delete: "Elimina",
  Language: "Lingua",
  "The interface only. Lyrics stay in the song's language":
    "Solo l'interfaccia. I testi restano nella lingua del brano",
  "A song is wrong or missing?": "Un brano è sbagliato o manca?",
  "Opens a ready-made report on GitHub with the song and what the app found":
    "Apre su GitHub una segnalazione già compilata con il brano e ciò che l'app ha trovato",
  "Send feedback": "Invia feedback",

  // Toasts
  "Could not change start at login": "Impossibile cambiare l'apertura all'avvio",
  "Lyrics cache cleared": "Testi salvati eliminati",
  "Saved lyrics were removed from this device.":
    "I testi salvati sono stati rimossi da questo dispositivo.",
  Playing: "In riproduzione",
  Paused: "In pausa",
  "Already used by {action}": "Già usata da: {action}",
  "Shortcut unavailable": "Scorciatoia non disponibile",
  "Another application may be using it.": "Un'altra applicazione potrebbe usarla.",
  "The shortcut could not be updated.": "Non è stato possibile aggiornare la scorciatoia.",
  "Pinned · {shortcut} to unpin": "Bloccato · {shortcut} per sbloccare",
  Unpinned: "Sbloccato",
  "Feedback ready on GitHub": "Feedback pronto su GitHub",
  "Check it and press Submit.": "Controllalo e premi Invia.",
  "Could not open GitHub": "Impossibile aprire GitHub",
  "Unknown track": "Brano sconosciuto",
  "No media session": "Nessuna sessione multimediale",
  "Unknown artist": "Artista sconosciuto",
  "Play something": "Riproduci qualcosa",
};

let current: Language = "en";

export function setLanguage(language: Language) {
  current = language;
  if (typeof document !== "undefined") document.documentElement.lang = language;
}

export function currentLanguage() {
  return current;
}

/**
 * The text in the interface's language. `{name}` in the text is replaced by the
 * value of that name.
 */
export function t(text: string, values: Record<string, string | number> = {}) {
  const translated = (current === "it" ? ITALIAN[text] : undefined) ?? text;
  return translated.replace(/\{(\w+)\}/g, (placeholder, name: string) =>
    name in values ? String(values[name]) : placeholder,
  );
}
