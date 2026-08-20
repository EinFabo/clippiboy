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
| Clip speichern (Paket-Ring → MP4, kein Re-Encode) | ✅ fertig |
| Globale Hotkeys | ✅ fertig |
| Clip-Player in der App | ✅ fertig |
| Tray-Symbol, Schließen ins Tray | ✅ fertig |
| Banner über dem Spiel | ✅ fertig |
| Spielerkennung (Prozess-EXE) | ✅ fertig |
| Einzelspuren neben dem Clip, Mischung nachträglich änderbar | ✅ fertig |
| Icon, Installer (NSIS), Auto-Update | ✅ fertig |
| Puffer-Automatik (beim Start / im Spiel) | ✅ fertig |
| Clip bearbeiten: Name, Beschreibung, Spurmischung, Zuschnitt | ✅ fertig |
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

Bricht der Typcheck mit `Inconsistency detected by ld.so` in einem
Build-Skript ab, liegt das Zielverzeichnis auf der Windows-Platte — WSL kann
von dort nicht jede Binärdatei starten. Dann einmal umlenken:

```bash
export CARGO_TARGET_DIR=~/.cache/clippiboy-target
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
  pipeline.rs            Capture → Encoder → Paket-Ring
  wgc.rs                 Windows.Graphics.Capture, Bilder als D3D11-Texturen
  gpu.rs                 D3D11-Gerät, von Capture und Encoder geteilt
  convert.rs             BGRA→NV12 auf der GPU + Taktgeber für echtes CFR
  mft.rs                 H.264-Encoder als Media Foundation Transform
  buffer.rs              keyframe-sicherer Paket-Ring (+ Unit-Tests)
  muxer.rs               Pakete + Tonspuren → MP4 (ffmpeg, ohne Re-Encode)
  stems.rs               Einzelspuren ablegen, entpacken, mischen (+ Tests)
  preview.rs             Wegwerf-Hilfsdateien (Wellenform für die Zeitleiste)
  audio/capture.rs       WASAPI je Quelle (Gerät, Loopback, Prozess)
  audio/engine.rs        laufende Streams, Pegel, Mischen
  audio/ring.rs          Ringpuffer je Quelle (+ Unit-Tests)
  audio/devices.rs       WASAPI-Endpunkte und Prozesse mit Audio-Session
  audio/mod.rs           Spur-Layout, Gain (+ Unit-Tests)
  capture.rs             Monitore und Fenster als Aufnahmeziele
  game.rs                Spielerkennung (+ Unit-Tests)
  games.json             EXE → Spielname
  tray.rs                Tray-Symbol, Menü, Tooltip
  overlay.rs             Overlay-Fenster ansteuern
  encode.rs              Encoder-Erkennung
  clips.rs               SQLite-Clip-Index
  config.rs              Konfiguration als JSON
  commands.rs            Tauri-Commands
  updater.rs             Update-Prüfung und Installation
src-tauri/examples/
  aufnahme-probe.rs      Capture → Encoder → Muxer einmal von Hand durchspielen
  spuren-probe.rs        zwei parallele Spurenabfragen auf denselben Clip
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

**Ein** Encoder läuft durch, und seine fertigen Pakete landen in einem Ring im
Arbeitsspeicher (`buffer.rs`). Beim Speichern werden die passenden Pakete
herausgeschnitten, als roher H.264-Elementarstrom abgelegt und mit dem Ton in
einem einzigen ffmpeg-Lauf zu einem MP4 gepackt (`-c:v copy`) — nichts wird neu
encodiert, ein Clip steht in ein bis zwei Sekunden.

Der Ring schneidet vorne immer auf ein Keyframe: Ein Clip, der mitten in einer
Bildgruppe anfinge, hätte am Anfang Klötzchen. Ältere Pakete fallen fortlaufend
weg, sodass genau die eingestellte Pufferlänge vorgehalten wird.

Das Bild geht als Direct3D-Textur direkt in den Hardware-Encoder — es wird nie
über die CPU kopiert. Capture und Encoder teilen sich dafür dasselbe D3D11-Gerät
(`gpu.rs`), die Umwandlung BGRA→NV12 macht der Video-Prozessor der Grafikkarte.

Vorher lag der Puffer als **MPEG-TS-Segmente** von 10 Sekunden auf der Platte,
die beim Speichern per `ffmpeg concat` zusammengesetzt wurden. Das hatte drei
Kosten, die alle weg sind:

* Jeder Segmentwechsel brauchte einen neuen Encoder. Der Aufbau dauert länger
  als ein Bildabstand, also musste der nächste im Hintergrund vorgebaut werden —
  und trotzdem riss an jeder Grenze ein Loch von 60 bis 300 ms ins Bild, das
  sich über einen Clip zum Versatz zwischen Bild und Ton summierte.
* Ein Segment, das keine Datei mehr hergab — etwa weil bei stehendem Bild kein
  einziges Frame ankam — brachte **jedes** weitere Speichern zum Scheitern,
  solange es im Ring lag: `Impossible to open '…/segment_001124.ts'`.
* Der Puffer stand ständig auf der Platte. Eine ältere Fassung, die abstürzte,
  ließ ihn dort liegen; beim ersten Start der neuen wird `buffer/` deshalb
  weggeräumt (auf einer Testmaschine 323 MB).

Ton und die Uhr des Puffers hängen an einem eigenen Thread, der alle 10 ms
läuft — nicht am Frame-Callback. Windows.Graphics.Capture liefert nämlich nur
bei Bildänderung ein Frame: hinge alles am Callback, würde bei ruhigem Bild der
Ton verhungern und der Puffer stehenbleiben. Aus demselben Grund taktet ein
eigener Faden die Bilder auf `1/fps` und schickt bei ruhigem Bild das letzte
noch einmal los — der Encoder sieht dadurch echtes CFR statt einer Bildrate,
die er selbst umrechnen müsste. Genau das war der Judder. Der Nullpunkt für
beide Spuren ist das erste eingetroffene Bild, damit Ton und Bild denselben
Zeitursprung haben.

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
* **Speichern** — rechnet die eingestellte Mischung in die Clipdatei. Der
  Zuschnitt ist bisher nur eine Markierung; die Datei bleibt in voller Länge.

Zwei Dinge, die man dabei wissen sollte:

**Der Clip hat genau eine Tonspur.** Discord, der Browser und die meisten
Player geben von einem MP4 stur die erste Tonspur wieder — lagen Mikrofon und
Discord wie früher als eigene Spuren daneben, waren sie überall außerhalb des
Editors stumm. Damit sich die Mischung trotzdem jederzeit ändern lässt, liegen
die rohen Einzelspuren daneben, je Clip ein Ordner unter
`%APPDATA%\ClippiBoy\tracks\<clip-id>\`. Sie gehören zum Clip und werden mit
ihm gelöscht.

**Die Vorschau kann nur leiser werden.** WebView2 kommt an `audioTracks` nicht
heran, deshalb lässt die UI die Einzelspuren als eigene Audioelemente synchron
zum Video mitlaufen. Deren Lautstärke lässt sich aber nur dämpfen, nie anheben:
steht ein Regler über 0 dB, senkt die Vorschau stattdessen die übrigen Spuren
ab. Die Balance stimmt damit, nur die Gesamtlautstärke liegt tiefer. Beim
Speichern wird der Pegel wirklich angehoben.
