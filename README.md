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

Hotkeys: `Strg+Shift+B` Puffer an/aus, `Strg+Shift+S` Clip speichern. Beide
lassen sich in den Einstellungen auf jede Taste legen — auch auf eine ohne
Zusatztaste, etwa `F9`. Eine einzelne Buchstabentaste gilt dann allerdings
überall, auch im Chat.

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
  components/ui/Menu.tsx      Rechtsklick-Menü (ein Menü, global)
  components/clipMenu.tsx     dessen Einträge für einen Clip
  components/TextMenu.tsx     WebView-Menü aus, eigenes Menü in Textfeldern
  components/SourceTrouble.tsx Tonquellen, die nicht oder doppelt laufen
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
  edit.rs                Clip neu schreiben: Mischung + Zuschnitt (+ Tests)
  preview.rs             Wegwerf-Hilfsdateien (Wellenform für die Zeitleiste)
  audio/capture.rs       WASAPI je Quelle (Gerät, Loopback, Prozess)
  audio/engine.rs        laufende Streams, Pegel, Mischen
  audio/ring.rs          Ringpuffer je Quelle (+ Unit-Tests)
  audio/devices.rs       WASAPI-Endpunkte und Prozesse mit Audio-Session
  audio/mod.rs           Spur-Layout, Gain (+ Unit-Tests)
  capture.rs             Monitore und Fenster als Aufnahmeziele, samt Hz
  game.rs                Spielerkennung (+ Unit-Tests)
  games.json             EXE → Spielname
  tray.rs                Tray-Symbol, Menü, Tooltip
  overlay.rs             Overlay-Fenster ansteuern
  encode.rs              Encoder-Erkennung
  clipboard.rs           Windows-Zwischenablage: Datei (CF_HDROP) und Text
  clips.rs               SQLite-Clip-Index
  filing.rs              Ordnung im Clip-Ordner: je Spiel ein Ordner (+ Tests)
  thumbs.rs              Vorschaubilder (im Datenverzeichnis, nicht beim Clip)
  config.rs              Konfiguration als JSON
  commands.rs            Tauri-Commands
  updater.rs             Update-Prüfung und Installation
src-tauri/examples/
  aufnahme-probe.rs      Capture → Encoder → Muxer einmal von Hand durchspielen
  spuren-probe.rs        zwei parallele Spurenabfragen auf denselben Clip
  schnitt-probe.rs       schneiden, nachmessen, aufheben, nachmessen
scripts/fetch-ffmpeg.mjs ffmpeg/ffprobe für das Paket holen
scripts/make-icons.py    alle Icon-Größen aus icons/icon.png
```

## Auflösung und Bildrate

Beides folgt der gewählten Quelle. Die Auflösung wird auf deren Höhe gedeckelt
und die Breite aus ihrem Seitenverhältnis gerechnet, nicht aus einem
angenommenen 16:9. Die Bildrate bietet die üblichen Stufen nur bis zur
Wiederholrate des Bildschirms an — und dessen eigene Rate obendrauf, damit ein
165-Hz-Panel auch wirklich 165 hergibt. Mehr Bilder aufzunehmen, als der
Bildschirm ausgibt, bringt keine Bewegung dazu, flüssiger zu sein; es entstehen
nur doppelte Bilder, die Bitrate kosten. Bei einem Fenster zählt der
Bildschirm, auf dem es liegt.

Der Kern rückt eine Einstellung, die nicht mehr passt, selbst zurecht
(`capture::fit_to_target`) — wer von einem 165-Hz-Monitor auf einen 60-Hz-
Zweitschirm wechselt, findet dort 60 vor statt einer Zahl, die das Panel nie
zeigen kann.

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

Eine Quelle mit eigener Spur behält diese Spur, solange sie eingeschaltet ist —
auch wenn sie stumm geschaltet oder eine andere auf Solo gestellt wird. Der
Mischer schiebt dann Stille hinein. Alles andere hieße, dass Stummschalten die
Quelle auch **rückwirkend** aus dem Puffer wirft, und dass jeder Schritt eines
Lautstärkereglers die bis dahin gepufferten Minuten dieser Spur kostet.

## Clips bearbeiten

Im Player öffnet **Bearbeiten** (oder `E`) einen Bereich neben dem Bild:

* **Name, Beschreibung, Spiel** — landen in der Clip-Datenbank, nicht im
  Dateinamen; die Datei behält ihren. Die Suche in der Galerie findet alle
  drei. Der Name lässt sich auch ohne Player ändern: In der Galerie ein Klick
  auf den Namen, er steht markiert da, Enter speichert, Escape verwirft. Wer das Spiel ändert, während die Galerie danach filtert, bleibt
  im Player trotzdem auf seinem Clip: Die Wiedergabeliste wird beim Öffnen
  eingefroren.
* **Tonspuren** — je Spur ein Regler von −30 bis +12 dB und ein Stummschalter.
* **Zuschnitt** — `I` und `O` setzen Anfang und Ende auf die aktuelle Stelle,
  die Griffe in der Zeitleiste lassen sich auch ziehen. Die Wiedergabe springt
  am Ende der Auswahl zurück an ihren Anfang.
* **Speichern** — schreibt beides in die Datei: die Mischung **und** den
  Zuschnitt. Was danach im Ordner liegt, ist der fertige Clip — man kann ihn
  ohne weiteres Zutun verschicken.

## Das Rechtsklick-Menü

Das eingebaute Menü von WebView2 — „Zurück", „Aktualisieren", „Drucken",
„Untersuchen" — ist in der ganzen App abgeschaltet. Es bietet keine einzige
nützliche Aktion und sieht aus wie ein Browser, der sich verlaufen hat.

An seine Stelle treten zwei eigene Menüs im Stil der Oberfläche:

**Auf einem Clip** (Kachel in der Galerie und Bild im Player): Öffnen · Mit
Standardplayer öffnen · Umbenennen · Favorit · **Clip kopieren** · Pfad
kopieren · Im Ordner zeigen · Löschen. „Clip kopieren" legt die **Datei** in
die Zwischenablage, nicht ihren Pfad — in Discord oder WhatsApp hängt Strg+V
den Clip danach als Anhang an, im Explorer legt es eine Kopie ab. Das Format
dafür ist `CF_HDROP`, und das kann kein WebView: Es kommt aus
`src-tauri/src/clipboard.rs`.

**In Textfeldern**: Ausschneiden · Kopieren · Einfügen · Alles markieren. Auch
der Text geht über den Kern statt über `navigator.clipboard` — Lesen aus der
Zwischenablage fragt im WebView um Erlaubnis, und dieser Dialog gehört nicht in
eine App, die ohnehin schon nativ ist. Eingefügt wird über den Setter des
Prototyps plus `input`-Ereignis, sonst bekäme React die Änderung nicht mit und
der Entwurf spränge beim nächsten Render zurück.

Das Menü nimmt bewusst keinen Fokus (`onMouseDown` abgefangen): Sonst verlöre
das Textfeld darunter seine Auswahl, und der Editor speicherte beim Blur mitten
im Vorgang. Die Tastatur (↑/↓/Enter/Escape) läuft deshalb über das Dokument.
Gerendert wird in `document.fullscreenElement ?? document.body` — im Vollbild
des Players ist alles andere unsichtbar.

## Ordnung im Clip-Ordner

Jedes Spiel bekommt seinen eigenen Ordner, Favoriten kommen in `Favoriten`,
und was kein Spiel hat, bleibt direkt im Clip-Ordner liegen:

```
Videos\ClippiBoy\
  clip_2026-08-18_11-37.mp4      ← ohne Spiel
  Bodycam\
  Counter-Strike 2\
  Favoriten\                     ← alles mit Herz, quer über die Spiele
```

Ändert sich das Spiel eines Clips oder sein Herz, wandert die Datei mit. Zwei
Regeln dazu:

**Verschoben wird erst, wenn der Clip nicht mehr offen ist.** Der Player hält
die Datei während der Wiedergabe; sie ihm unter den Füßen wegzuziehen, ließe
das Video abreißen. Die Galerie holt es nach, sobald der Player zugeht — und
was dabei schiefging (Datei gesperrt, Absturz), räumt `filing::tidy` beim
nächsten Start auf. Die Galerie stimmt in der Zwischenzeit trotzdem: Sie liest
aus der Datenbank, nicht aus dem Dateisystem.

**Angefasst wird nur, was im eingestellten Clip-Ordner liegt** — direkt darin
oder eine Ebene tiefer. Wer den Speicherort umstellt, lässt seine bisherigen
Clips bewusst liegen, wo sie sind; die zieht niemand hinterher. Leer gewordene
Spielordner verschwinden von selbst, der Clip-Ordner selbst nie.

Ein **Favorit ist zugleich eine Kategorie**: Die Datei liegt in `Favoriten`, in
der App bleibt der Clip unter seinem Spiel auffindbar — beides sind Filter über
dieselbe Datenbank. Das Herz sitzt auf der Kachel (sichtbar, sobald es gesetzt
ist) und im Player.

Spielnamen werden für den Ordner entschärft: verbotene Zeichen fliegen raus,
Punkte und Leerzeichen am Ende auch, Gerätenamen wie `CON` bekommen einen
Unterstrich davor, und nach 60 Zeichen ist Schluss — Spielnamen kommen teils
aus Fenstertiteln, und die können ganze Sätze sein.

Die **Vorschaubilder** liegen nicht beim Clip, sondern unter
`%APPDATA%\ClippiBoy\thumbs\<clip-id>.jpg`. Der Clip-Ordner gehört dem
Nutzer und soll nur Videos enthalten — wer ihn öffnet, will Clips sehen und
nicht zu jedem eine halbe Bilddatei. Bilder aus älteren Fassungen, die noch
neben dem Video liegen, zieht ClippiBoy beim Start dorthin um. Nach einem
Schnitt wird das Bild neu gerechnet, mit dem Clip wird es gelöscht.

Der Zuschnitt geht dabei nicht verloren. Beim ersten echten Schnitt wandert die
unversehrte Aufnahme nach `%APPDATA%\ClippiBoy\originals\<clip-id>\`, und im
Bearbeiten-Bereich steht dann **Zuschnitt aufheben** — ein Klick, und der ganze
Clip ist zurück. Weiter *hinein*schneiden geht auch ohne Aufheben; die Griffe
laufen dabei über die Zeitachse der geschnittenen Datei, gerechnet wird intern
im Original.

Vier Dinge, die man dabei wissen sollte:

**Hinten kürzen ist verlustfrei, vorne nicht.** Fängt der Schnitt bei null an,
wird das Bild nur kopiert (`-c:v copy`) und die Datei steht in ein bis zwei
Sekunden. Ein Schnitt am Anfang muss dagegen bildgenau sitzen — beim Kopieren
rutschte er auf das Keyframe davor, also bis zu zwei Sekunden zu früh. Dafür
wird das Bild neu encodiert, mit dem Encoder aus den Einstellungen und x264 als
Rückfall, falls die Hardware streikt (etwa weil nebenan der Puffer läuft). Ein
Fortschrittsbalken zeigt, wie weit es ist. Gerechnet wird dabei **immer** aus
dem Original, nie aus der schon geschnittenen Datei — der Verlust bleibt so bei
einer Generation, auch wenn man dreimal nachschneidet.

**Ein geschnittener Clip braucht doppelt Platz**, solange das Original daneben
liegt. Es verschwindet, sobald der Zuschnitt aufgehoben oder der Clip gelöscht
wird.

**Der Clip hat genau eine Tonspur.** Discord, der Browser und die meisten
Player geben von einem MP4 stur die erste Tonspur wieder — lagen Mikrofon und
Discord wie früher als eigene Spuren daneben, waren sie überall außerhalb des
Editors stumm. Damit sich die Mischung trotzdem jederzeit ändern lässt, liegen
die rohen Einzelspuren daneben, je Clip ein Ordner unter
`%APPDATA%\ClippiBoy\tracks\<clip-id>\`. Sie gehören zum Clip und werden mit
ihm gelöscht.

Die Spuren bleiben dabei **ungeschnitten** und stehen immer in Koordinaten der
unversehrten Aufnahme. Das ist Absicht: Sie werden dadurch nie ersetzt, es gibt
keinen zweiten Zeitstrahl, der davonlaufen kann, und Windows kann einem keine
offene Datei sperren. Der Preis ist eine Zahl, die stimmen muss — der Versatz
zwischen Spur und Bild, und das ist genau `original.startMs`.

**Die Vorschau mischt über WebAudio.** WebView2 kommt an `audioTracks` nicht
heran, deshalb lässt die UI die Einzelspuren als eigene Audioelemente synchron
zum Video mitlaufen. Deren `volume` kann nur dämpfen, nie anheben — jeder
Regler über 0 dB hätte die übrigen Spuren abgesenkt statt seine eigene
anzuheben, und wer am Mikrofon drehte, hörte alles andere lauter oder leiser
werden. Die Spuren laufen deshalb über einen kleinen WebAudio-Graphen: je Spur
ein `GainNode`, dahinter ein Master und ein hartes Begrenzen auf ±1 — dasselbe,
was beim Speichern passiert. Vorschau und fertiger Clip klingen damit gleich.

Die Spuren liegen unter `asset.localhost` und damit auf einer anderen Herkunft
als die Oberfläche; ohne `crossOrigin = "anonymous"` gäbe ein
`MediaElementSource` **Stille** aus, ohne jede Fehlermeldung. Für den Fall, dass
es trotzdem einmal so kommt, hört ein `AnalyserNode` mit und fällt nach zwei
Sekunden ohne ein einziges Sample auf den alten Weg über `volume` zurück.
