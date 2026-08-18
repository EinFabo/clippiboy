# ClippiBoy

Clip-Recorder für Windows: Replay-Puffer wie bei Medal, aber mit einem
Audio-System, das mehrere Quellen gleichzeitig kann — Ausgabegeräte, einzelne
Anwendungen (Prozess-Loopback) und Mikrofone, jede Quelle wahlweise im Hauptmix
oder auf einer eigenen Tonspur im Clip.

## Stand

| Bereich | Stand |
|---|---|
| UI-Shell, Design-System, alle Screens | ✅ fertig |
| Geräte-, Prozess- und Monitor-/Fenster-Enumeration | ✅ fertig (Windows) |
| Encoder-Erkennung über DXGI (NVENC/AMF/QSV/x264) | ✅ fertig |
| Konfiguration + Clip-Datenbank (SQLite) | ✅ fertig |
| Replay-Ring-Puffer (keyframe-sicher, getestet) | ✅ fertig |
| Video-Capture (Windows.Graphics.Capture, zero-copy) | ✅ fertig |
| Encoder (Media Foundation, H.264 + AAC) | ✅ fertig |
| WASAPI-Aufnahme aller Quellentypen + Mixer | ✅ fertig |
| Clip speichern (TS-Segmente → MP4, kein Re-Encode) | ✅ fertig |
| Globale Hotkeys | ✅ fertig |
| Clip-Player in der App | ✅ fertig |
| Tray-Symbol, Schließen ins Tray | ✅ fertig |
| Banner über dem Spiel | ✅ fertig |
| Spielerkennung (Prozess-EXE) | ✅ fertig |
| Zusätzliche Tonspuren im Clip | ✅ implementiert, noch ungetestet |
| Icon, Installer (NSIS), Auto-Update | ✅ fertig |
| Puffer-Automatik (beim Start / im Spiel) | ✅ fertig |
| Clip bearbeiten: Name, Beschreibung, Spurmischung, Zuschnitt, Export | ✅ fertig |
| Upload | ⏳ offen |

## Entwickeln

Gebaut wird **auf der Windows-Seite** (WSL kann keine Windows-Binaries bauen).
In PowerShell:

```powershell
cd C:\Users\fabia\projects\clippiboy
npm install
npm run ffmpeg       # holt ffmpeg.exe/ffprobe.exe nach src-tauri\resources
npm run app          # Tauri-Dev-Modus mit Hot Reload
npm run app:build    # NSIS-Installer nach src-tauri\target\release\bundle
```

Voraussetzungen: Node 20+, Rust (MSVC-Toolchain), Visual Studio 2022 Build
Tools mit C++-Workload und WebView2 (ab Windows 11 vorinstalliert).

ffmpeg muss **nicht** installiert sein: `npm run ffmpeg` legt eine
getestete Fassung neben die App, und die hat Vorrang vor einer im PATH.

Hotkeys: `Strg+Shift+B` Puffer an/aus, `Strg+Shift+S` Clip speichern.

Das ✕ schließt die App nicht, sondern legt sie ins Tray — dort lässt sie sich
wieder öffnen, der Puffer an- und ausschalten und ein Clip speichern; der
Tooltip zeigt Pufferstand und erkanntes Spiel. Beendet wird über „Beenden" im
Tray-Menü.

### Nur die UI (auch unter Linux/WSL möglich)

```bash
npm run dev          # http://localhost:1420 mit Mock-Daten aus src/lib/mock.ts
```

### Prüfen ohne Windows

```bash
cd src-tauri
cargo test --lib                            # Puffer-, Mixer- und DB-Logik
cargo check --target x86_64-pc-windows-gnu  # typprüft auch den Windows-Code
```

## Weitergeben und aktualisieren

`npm run app:build` erzeugt `ClippiBoy_<version>_x64-setup.exe` — eine Datei,
die man verschicken kann. Sie installiert pro Benutzer (kein Administrator
nötig), legt einen Startmenü-Eintrag an und bringt ffmpeg mit; auf der
Gegenseite braucht es nur WebView2, das auf Windows 10/11 vorhanden ist.
Dadurch, dass ffmpeg mit im Paket steckt, ist das Setup rund 90 MB groß.

**Updates** kommen aus den GitHub Releases von `EinFabo/clippiboy`. Jedes Paket
ist signiert; der öffentliche Schlüssel steht in `tauri.conf.json`, der private
liegt unter `%USERPROFILE%\.clippiboy\updater.key` samt Passwort daneben in
`updater.password`. Beides gehört als Repository-Secret hinterlegt und **nicht**
ins Repo:

| Secret | Inhalt |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | Inhalt von `updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Inhalt von `updater.password` |

Geht der Schlüssel verloren, kann keine bestehende Installation mehr ein Update
annehmen — dann bleibt nur, allen ein neues Setup zu schicken.

Veröffentlichen:

```powershell
# 1. Version in src-tauri/tauri.conf.json und package.json hochzählen
# 2. Tag setzen und schieben — der Workflow baut, signiert und veröffentlicht
git tag v0.2.0
git push origin v0.2.0
```

Lokal von Hand bauen und signieren (das Passwort muss gesetzt sein, sonst
fragt der Build interaktiv danach):

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "$env:USERPROFILE\.clippiboy\updater.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Get-Content "$env:USERPROFILE\.clippiboy\updater.password"
npm run app:build
```

Die App fragt beim Start einmal nach, ob es etwas Neueres gibt, installiert
aber nichts von selbst: Unter Windows heißt Installieren, dass sich die App
beendet und das Setup startet — mitten in einer Aufnahme wäre das genau das
Falsche. Der Fund landet in den Einstellungen unter „Version", dort läuft die
Installation auf Knopfdruck (ein laufender Puffer wird vorher sauber
gestoppt).

## Wann der Puffer läuft

Standardmäßig gar nicht von selbst — der Puffer geht über Hotkey, Tray oder
den Knopf in der App an. In den Einstellungen unter „Verhalten" lässt sich das
umstellen:

* **Puffer automatisch einschalten** — die Automatik übernimmt.
* zusätzlich **Nur im Spiel puffern** an: der Puffer startet, sobald ein Spiel
  im Vordergrund erkannt wird, und stoppt 30 Sekunden nachdem keines mehr da
  ist. Die halbe Minute Nachlauf ist Absicht: kurzes Alt-Tab darf den
  Mitschnitt nicht abwürgen.
* zusätzlich **Nur im Spiel puffern** aus: der Puffer läuft ab dem Start von
  ClippiBoy durch, egal was im Vordergrund ist.

Was der Nutzer selbst schaltet, hat immer Vorrang: Ein von Hand gestarteter
Puffer wird nie automatisch gestoppt, und ein von Hand gestoppter nicht zwei
Sekunden später wieder gestartet — die Automatik hält sich zurück, bis das
Spiel beendet ist.

Mit **Mit Windows starten** trägt sich ClippiBoy in den Autostart ein und
startet dann versteckt im Tray (`--autostart`). Zusammen mit der Automatik
läuft der Mitschnitt damit, ohne dass man je ein Fenster sieht.

## Aufbau

```
src/                     React-UI
  lib/types.ts           IPC-Typen — Gegenstück zu src-tauri/src/model.rs
  lib/ipc.ts             typisierte Command-Wrapper
  lib/mock.ts            Mock-Daten für den Browser-Modus
  store.ts               Zustand-Store, spricht mit dem Kern
  routes/                Übersicht, Clips, Audio-Mixer, Aufnahme, Einstellungen
  components/ClipPlayer.tsx   Player über der Galerie
  components/ClipEditor.tsx   Bearbeiten-Bereich: Metadaten, Spuren, Export
  lib/useClipMix.ts      zusätzliche Tonspuren synchron zum Video abspielen
  overlay/               eigenes Fenster: der Banner über dem Spiel
src-tauri/src/
  model.rs               gemeinsame Datentypen
  pipeline.rs            Capture → Encoder → Segment-Ring
  muxer.rs               Segmente + Tonspuren → MP4 (ffmpeg, ohne Re-Encode)
  audio/capture.rs       WASAPI je Quelle (Gerät, Loopback, Prozess)
  audio/engine.rs        laufende Streams, Pegel, Mischen
  audio/ring.rs          Ringpuffer je Quelle (+ Unit-Tests)
  audio/devices.rs       WASAPI-Endpunkte und Prozesse mit Audio-Session
  audio/mod.rs           Spur-Layout, Gain (+ Unit-Tests)
  buffer.rs              keyframe-sicherer Paket-Ring (+ Unit-Tests)
  capture.rs             Monitore und Fenster als Aufnahmeziele
  game.rs                Spielerkennung (+ Unit-Tests)
  games.json             EXE → Spielname
  tray.rs                Tray-Symbol, Menü, Tooltip
  overlay.rs             Overlay-Fenster ansteuern
  encode.rs              Encoder-Erkennung
  clips.rs               SQLite-Clip-Index
  export.rs              Spuren lesen/entpacken, Clip neu ausgeben (+ Tests)
  config.rs              Konfiguration als JSON
  commands.rs            Tauri-Commands
  updater.rs             Update-Prüfung und Installation
scripts/fetch-ffmpeg.mjs ffmpeg/ffprobe für das Paket holen
scripts/make-icons.py    alle Icon-Größen aus icons/icon.png
```

## Spielerkennung

Erkannt wird über den **Prozess** hinter dem Vordergrundfenster, nicht über
dessen Titel: Titel ändern sich im Spiel, und viele Spiele setzen gar keinen.
Steht die EXE in `src-tauri/src/games.json`, ist der Name damit sicher. Eine
unbekannte Anwendung gilt als Spiel, wenn ihr Fenster den ganzen Monitor
ausfüllt — dann wird der aufgeräumte Fenstertitel benutzt.

Eigene Namen ohne Neubau: eine `games.json` nach `%APPDATA%\ClippiBoy\` legen,
sie wird über die eingebaute Liste gelegt.

```json
{ "meinspiel.exe": "Mein Spiel" }
```

Nachgesehen wird alle zwei Sekunden. Beim Speichern zählt das Spiel, das
*während des Pufferns* lief — sonst stünde am Clip „ClippiBoy", wenn man ihn
über den Knopf im Fenster speichert.

## Der Banner über dem Spiel

Ein zweites, durchsichtiges Fenster, das immer oben liegt, keinen Fokus annimmt
und keine Mausklicks abfängt. Es erscheint auf dem Monitor, auf dem gerade
gespielt wird, und meldet gespeicherte Clips, Puffer an/aus und Fehler — jedes
davon in den Einstellungen einzeln abschaltbar.

Der Banner klebt auf einem festen Bildschirm (einstellbar, Standard: der
primäre), damit er nicht zwischen Monitoren springt; wahlweise folgt er dem
Fenster im Vordergrund. Die Einblendung ist eine Umrandung, die sich einmal um
die Karte zieht und danach ausglüht — reine Compositor-Animation, damit sie
nicht ruckelt, wenn das Spiel die GPU braucht.

Über einem Spiel im **exklusiven** Vollbild kann er nicht erscheinen: dafür
bräuchte es einen Present-Hook im Spielprozess, und genau das macht ClippiBoy
bewusst nicht. Im randlosen Vollbild und im Fenstermodus — also bei praktisch
allen aktuellen Spielen — funktioniert es.

## Wie der Replay-Puffer funktioniert

Aufgenommen wird durchgehend in **MPEG-TS-Segmente** von 10 Sekunden
(`%APPDATA%\ClippiBoy\buffer`). TS ist der einzige gängige Container, der sich
verlustfrei aneinanderhängen lässt — beim Speichern werden also nur die
passenden Segmente kopiert und in ein MP4 umgepackt (`-c copy`), ohne neu zu
encodieren. Ein Clip steht dadurch in ein bis zwei Sekunden.

Ältere Segmente werden fortlaufend gelöscht, sodass immer genau die
eingestellte Pufferlänge vorgehalten wird. Beim Stoppen wird alles aufgeräumt.

Das Bild geht als Direct3D-Textur direkt in den Hardware-Encoder — es wird nie
über die CPU kopiert.

Ton, Segmentwechsel und die Uhr des Puffers hängen an einem eigenen Thread, der
alle 10 ms läuft — nicht am Frame-Callback. Windows.Graphics.Capture liefert
nämlich nur bei Bildänderung ein Frame: hinge alles am Callback, würde bei
ruhigem Bild der Ton verhungern und der Puffer stehenbleiben. Der Nullpunkt für
beide Spuren ist das erste eingetroffene Bild, damit Ton und Bild denselben
Zeitursprung haben.

Der Encoder für das nächste Segment wird im Hintergrund vorgebaut. Ihn im
Capture-Thread aufzusetzen dauert länger als ein Bildabstand und ließ die
Aufnahme früher alle 10 Sekunden hängen.

Die Ringpuffer der Audioquellen laufen ab Programmstart mit (für die
Pegelanzeige), werden aber vor jeder Aufnahme geleert und währenddessen auf
40 ms Rückstand begrenzt. Ohne das stünde in ihnen alter Ton, den nie jemand
abgeholt hat — der Clip liefe von der ersten Sekunde an hinter dem Bild her.

## Das Audio-System

Drei Quellentypen, beliebig kombinierbar:

* **Ausgabegerät (Loopback)** — kompletter Ton eines Endpunkts
* **Anwendung** — Prozess-Loopback über `ActivateAudioInterfaceAsync`
  (Windows 10 Build 20348+), z.B. Discord getrennt vom Spiel
* **Eingabegerät** — Mikrofon

Jede Quelle hat Gain, Mute, Solo und Live-Pegel. Quellen ohne „eigene Spur"
laufen in den Hauptmix, die anderen werden parallel als PCM mitgeschrieben und
beim Speichern als zusätzliche Tonspuren ins MP4 gemuxt — so lässt sich
z.B. Discord im Schnitt nachträglich stummschalten. Ohne Schnittprogramm geht
das ebenso: siehe „Clips bearbeiten".

## Clips bearbeiten

Im Player öffnet **Bearbeiten** (oder `E`) einen Bereich neben dem Bild:

* **Name, Beschreibung, Spiel** — landen in der Clip-Datenbank, nicht im
  Dateinamen; die Datei bleibt, wo sie ist. Die Suche in der Galerie findet
  alle drei.
* **Tonspuren** — je Spur ein Regler von −30 bis +12 dB und ein Stummschalter.
* **Zuschnitt** — `I` und `O` setzen Anfang und Ende auf die aktuelle Stelle,
  die Griffe in der Zeitleiste lassen sich auch ziehen. Die Wiedergabe springt
  am Ende der Auswahl zurück an ihren Anfang.
* **Exportieren** — schreibt eine neue Datei, in der alle Spuren mit ihren
  Reglern zu **einer** Tonspur zusammengerechnet sind.

Zwei Dinge, die man dabei wissen sollte:

**Die Vorschau kann nur leiser werden.** WebView2 gibt von einem MP4 immer nur
die erste Tonspur wieder — an `audioTracks` kommt man nicht heran. Die übrigen
Spuren entpackt der Kern deshalb einzeln nach `%APPDATA%\ClippiBoy\preview`
und die UI lässt sie als eigene Audioelemente synchron mitlaufen. Deren
Lautstärke lässt sich aber nur dämpfen, nie anheben: steht ein Regler über
0 dB, senkt die Vorschau stattdessen die übrigen Spuren ab. Die Balance stimmt
damit, nur die Gesamtlautstärke liegt tiefer. Der Export hebt den Pegel wirklich
an.

**Ohne Schnitt am Anfang bleibt das Bild unangetastet.** Dann wird nur der Ton
neu gerechnet (`-c:v copy`), und der Export ist in Sekunden fertig. Ein Schnitt
am Anfang muss bildgenau sitzen, sonst rutschte er auf das nächste Keyframe —
dafür wird das Bild neu encodiert, mit dem Encoder aus den Einstellungen und
x264 als Rückfall, falls die Hardware streikt.
