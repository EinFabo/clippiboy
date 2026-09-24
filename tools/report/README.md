# ClippiBoy — Diagnosebericht

Ein Programm, das einmal ausgeführt wird und alles, was zur Fehlersuche nötig
ist, in **eine** Datei auf dem Desktop schreibt: `clippiboy-bericht.txt`.

Gedacht für den Fall, dass der Mensch, der den Fehler sieht, nicht der Mensch
ist, der ein Log lesen kann. Er startet es, schickt die eine Datei zurück.

```
npm run report
```

baut `tools/report/target/release/clippiboy-report.exe` — eine einzelne Exe von
rund 290 KB, ohne Abhängigkeiten, in wenigen Sekunden gebaut. Die kann man
verschicken; sie braucht nichts weiter auf dem Zielrechner.

## Was drinsteht

- System, Grafikkarten samt Treiberdatum, Bildschirme mit ihrer Anordnung
- Installierte Version, ob sie gerade läuft und ob sie antwortet
- Autostart-Eintrag in der Registry und im Autostart-Ordner
- `config.json` vollständig — **ohne** den Stream-Deck-Token
- Welche ffmpeg-Version geholt wurde
- Alle Abstürze und Hänger aus dem Windows-Ereignisprotokoll der letzten 30 Tage
- **Ob das Bild stehengeblieben ist** — siehe unten
- Die Logs: alle Ereigniszeilen, dazu die letzten Pipeline-Zeilen

## Die Frage nach dem stehenden Bild

Windows.Graphics.Capture liefert nur dann ein Bild, wenn sich auf dem Schirm
etwas geändert hat. Ein Bildschirm, auf dem nichts passiert, erzeugt deshalb
minutenlang nur Wiederholer — völlig normal, besonders wenn aufgenommen und
gearbeitet auf verschiedenen Schirmen wird.

Der Unterschied liegt nicht im Stehen, sondern im **Nicht-Wiederkommen**. Eine
Aufnahme, die an einem toten Schirm hängt, erholt sich nie mehr; ihre Strecke
läuft bis ans Ende des Logs. Genau das sagt der Bericht in seiner letzten Zeile
zu jedem Log — alles andere darüber ist nur Material.

## Es liest nur

Nichts wird installiert, nichts geändert, nichts verschickt. Die Datei landet
auf dem Desktop, und wer sie erzeugt hat, entscheidet, was damit geschieht. Der
Token des Steuerungs-Ports wird auf dem Weg herausgenommen: ein Schlüssel, der
durch einen Chat wandert, ist ein weggegebener Schlüssel.
