# Stale Uninstaller – design för plan 4

Datum: 2026-10-18
Status: godkänd 2026-10-18. Licens: MIT.
Bygger på: `docs/design/2026-10-17-stale-uninstaller-design.md` (huvuddesignen, plan 1–3 är byggda), `docs/research/2026-10-18-junk-file-catalogue.md` (skräpkatalog med mätningar), `docs/research/2026-10-18-parallel-fs-scan.md`.

## 1. Avsikt och varför

Plan 1–3 hjälper användaren att hitta och ta bort **program**. Plan 4 hjälper användaren att förstå och rensa **filer och mappar** – särskilt i AppData och ProgramData – där det svåra inte är att hitta dem utan att veta *vad de är, vem som äger dem och om de går att ta bort utan att något går sönder*. Användarens ord: "det är ofta filer i appdata som jag inte vet vad de är till för", "svårt att veta vad som är tryggt att ta bort", "man minns inte att man installerade det", och "programmet bör vara lite proaktivt att välja åt en vad som är skräpfiler förmodligen, och förklara varför".

Plan 4 rättar också ett verkligt fel som syns på utvecklarens dator: 173 av 286 program visas som "Aldrig sedd" fast Windows har körspår för dem (Chrome, Steam, Discord, Visual Studio). Och plan 4 gör programmet möjligt att ladda ner: en installer och en GitHub-release.

## 2. Mål, icke-mål, anti-mål

### Mål

1. **"Senast använd" stämmer på riktiga datorer.** Matchningsfelet rättas och regeln om delkomponenter införs (§3).
2. **Mappförklararen** för AppData och ProgramData: ägare, ägarens aktivitet, innehållstyp, om det byggs upp igen, dom med "varför" (§4).
3. **Proaktiva förslag**: Stale bockar själv i det som är tryggt skräp, med en sammanfattning av hur mycket som kan frigöras utan risk (§5).
4. **Skräpletaren**: kända skräpplatser från researchen (Temp, shader-cacher, webbläsar- och Electron-cacher, utvecklarcacher, kraschdumpar, Windows Update-rester, `Windows.old`) (§6).
5. **"Vart tog utrymmet vägen?"**: ögonblicksbilder av stora mappar och en vy över vad som växt (§7).
6. **Programmens ursprung** i detaljpanelen: utgivare/signatur, installationsdatum, installationskälla, beskrivning (§8).
7. **Krympa Docker- och WSL-diskar** (§9).
8. **Installer och release** (§10).
9. **Parkerade fel från plan 2–3** (§11).

### Icke-mål

- Rensa webbläsarprofilers data (kakor, lösenord, historik) – bara cacher.
- Rensa `Windows\Installer`, `WinSxS` för hand, `Package Cache`, Prefetch, pagefile/hiberfil, Store-appars `Packages` – de visas med förklaring men erbjuds aldrig för borttagning.
- Uppslag av okända mappar på internet (skickar mappnamn till extern tjänst). Kan bli ett tillval senare.
- Microsoft Store-appar som program (version 3).
- Kodsignering av installern i v1 (kostar; varningen dokumenteras).

### Anti-mål

- Något som används förbockas. En mapp vars ägare använts nyligen och som innehåller inställningar eller data förbockas aldrig.
- En dom utan förklaring. Varje dom har ett "varför" i en mening.
- Något raderas direkt. Allt filbaserat går via karantänen (plan 2). Undantag med egen bekräftelse: åtgärder som körs via Windows eller ett verktygs eget kommando (§6.3, §9), eftersom de inte går att ångra.
- Okänt behandlas som skräp. Okända mappar förbockas aldrig.

## 3. "Senast använd" som stämmer

### 3.1 Matchningsfelet

Upphöjd `dump` på utvecklarens dator: Prefetch har 490 filer och BAM 151 värden (bl.a. `chrome.exe`, `steam.exe`, `link.exe`), men bara 3 program får datum från Prefetch, 17 från BAM, och Chrome, Steam och Discord visas som aldrig sedda. Första steget är ett diagnosläge `dump usage <namn>` som skriver ut varje kandidat-exe för programmet, varje träff från varje källa, och varför den räknas eller inte. Felet rättas utifrån vad diagnosen visar; verifieras mot minst Chrome, Steam, Discord och Visual Studio på utvecklarens dator. Upphöjda körningar görs via den schemalagda uppgiften `StaleDevElevated`, som kör alla skript i `.superpowers/elevated/jobs/` med höjd behörighet och loggar till `.superpowers/elevated/log.txt` (start: `schtasks /run /tn StaleDevElevated` eller `Start-ScheduledTask`). Diagnos och rättning kan därför ske i samma plan.

### 3.2 Delkomponenter räknas

Beslutat i samtalet: ett program räknas som använt när **någon** av dess exe-filer har körts, inte bara huvudprogrammet. Uppdaterare, tjänster, kraschrapportörer, tray-hjälpare och avinstallerare räknas fortfarande inte (samma namnregler som i plan 1, plus registrerade tjänster). Kolumnen visar källan i parentes när det inte var huvudprogrammet: "Idag (via link.exe)". Verifierat underlag: på utvecklarens dator har Prefetch `LINK.EXE` (idag), `VCTIP.EXE`, `MSBUILD.EXE` under Visual Studios installationsmapp.

Förtydligat under genomförandet (2026-10-18):
- Bara exe-filer i programmets egen installationsmapp räknas som delkomponenter. En träff som hör till programmet bara via filnamnet räknas bara om den är huvudprogrammet.
- Ett program som autostartar räknas som använt, eftersom det körs. En schemalagd uppgift som startar programmets eget huvudprogram räknas.
- Schemalagda underhållsjobb som inte är huvudprogrammet räknas inte, till exempel Office ActionsServer och Firefox default-browser-agent.
- Tjänster räknas aldrig, inte ens när tjänsten är programmets ikonfil (Office ClickToRun).
- En installationsfil räknas bara när den är programmets eget huvudprogram och körningen syns i Prefetch eller BAM. Ett program vars enda spår är dess egen cachade installationsfil räknas som aldrig använt.
- En undantagen hjälpfil som användaren själv har startat från Start eller skrivbordet räknas, men dateras bara efter de egna starterna.

### 3.3 Beroenden

Program som andra program behöver – SDK:er, runtimes, Visual C++-redistributables, .NET, Windows SDK, Python/Node/Java när andra program pekar dit – märks "används av andra program" i listan och kräver en extra bekräftelse innan de läggs i avinstallationskön. Byggarens val hur beroenden upptäcks (namnmönster + referenser från andra programs mappar är en rimlig början).

## 4. Mappförklararen

En ny vy "Mappar" som listar de direkta undermapparna i `%APPDATA%`, `%LOCALAPPDATA%`, `%USERPROFILE%\AppData\LocalLow` och `%ProgramData%` (för aktuell användare; andra profiler visas bara som "annan användare", förbockas aldrig), med en nivå djupare i kända behållare (t.ex. `Google`, `Microsoft`, utgivarmappar).

För varje mapp visas:

| Fråga | Svar |
|---|---|
| Vem äger den? | Installerat program (namn från programlistan), avinstallerat program (koppling till spökprogrammen), Windows, eller okänt |
| Hur aktivt är ägaren? | Använt idag / för N dagar sedan / aldrig / borta – från "senast använd" |
| Vad innehåller den? | Cache, loggar, kraschdumpar, nedladdade uppdateringar, inställningar, sparfiler/användardata, blandat |
| Kommer den tillbaka? | "Byggs upp igen" eller "försvinner för gott" |
| Storlek, senast ändrad | Mätt (i bakgrunden, parallellt, se §12) |
| Dom | Säker / Byggs upp igen / Försiktig / Rör ej, alltid med en mening "varför" |

### 4.1 Hur ägaren avgörs

I ordning: (1) en medföljande **katalog** (`catalogue.json` inbyggd i programmet) med kända mappar → ägare, innehållstyp, dom och förklaring, **skriven själv** (beslutat: projektet är MIT-licensierat; BleachBits GPL-definitioner och övrig research används bara som vägvisare till vilka mappar som finns, ingenting kopieras); (2) koppling till programlistan (namn, utgivare, installationsmapp – samma namnregler som restsökningen); (3) koppling till spökprogrammen; (4) **ledtrådar** när inget matchar.

### 4.2 Hur innehållstypen avgörs

Från katalogen när den finns. Annars av innehållet (mitt val, efter granskning: innehållsreglerna kan förbocka, men bara undermappar – se §5 – och vyn har därför valbara underrader för "cachedelen" av en mapp): undermappar med namn som `Cache`, `Code Cache`, `GPUCache`, `ShaderCache`, `DawnCache`, `CrashDumps`, `logs`, `Temp`, filtyper (`.log`, `.dmp`, `.tmp`, `.etl`) → cache/loggar; `Saves`, `profiles`, `.db`/`.sqlite` i roten → data. Electron-appar känns igen på kombinationen `Cache` + `Code Cache` + `GPUCache` + `Local Storage`; i dem är bara cachemapparna skräp, aldrig `Local Storage`/`IndexedDB`.

### 4.3 Okända mappar

Beslutat i samtalet: visa ledtrådarna (storlek, senast ändrad, filtyper, största filer, vilka program/registervärden som refererar till sökvägen) och märk "Okänd – avgör själv". Förbockas aldrig.

## 5. Proaktiva förslag

Överst i Mappar- och Skräp-vyerna: "**N GB kan rensas utan risk**" med en knapp som visar förslagen. Förbockade är bara:

- **katalogposter** med dom Säker eller Byggs upp igen (cacher, temp, loggar) – oavsett ägarens aktivitet, och bara när ägarprogrammet inte körs (kontrolleras vid borttagning),
- mappar som tillhör avinstallerade program (spökprogram med dom "troligen avinstallerat"),
- **innehållsigenkända** cache- och loggundermappar (t.ex. `Cache`, `GPUCache`, `logs`) i mappar vars ägare är känd och inte använts på över 6 månader.

I mappar med okänd ägare förbockas ingenting, inte heller igenkända undermappar (mitt val).

**Summan "N GB kan rensas utan risk"** räknar bara det som går via karantänen. Verktygs- och Windows-åtgärder (§6.3, §9) visas separat under "Mer att frigöra med Windows eller verktygens egna kommandon", eftersom de inte går att ångra (mitt val).

**När utrymmet frigörs** (mitt val): skräp (katalogposter och igenkända cacher) läggs i karantänen med kort hålltid, **3 dagar** i stället för de vanliga 30, och resultatvyn har knappen "Töm nu" för den omgången. Texten säger "frigörs när karantänen töms (automatiskt efter 3 dagar)". Karantänmanifestet får ett fält för hålltid per post. Temp-filer läggs som en post per toppmapp eller som en samlad post per körning, inte en post per fil (byggarens val inom det).

Aldrig förbockade: inställningar och data för program som använts senaste 6 månaderna, okända mappar, allt med dom Försiktig eller Rör ej, andra användares mappar. Varje förbockad rad visar sitt "varför". Allt går via karantänen.

## 6. Skräpletaren

### 6.1 Kategorier i v1

Från researchen (siffror mätta på utvecklarens dator):

| Kategori | Exempel | Mätt | Dom | Åtgärd |
|---|---|---|---|---|
| Temp | `%TEMP%`, `C:\Windows\Temp` – filer äldre än 7 dagar som inte är låsta | 5,7 GB | Säker | Karantän |
| Shader-cacher | AMD `DxCache`, NVIDIA `DXCache`, `D3DSCache` | ~2,0 GB | Byggs upp igen | Karantän |
| Webbläsar-/Electron-cacher | Chrome/Edge `Cache`, Discord/VS Code/Teams `Cache`/`Code Cache`/`GPUCache`, Steam `htmlcache` | ~1,3 GB | Byggs upp igen (bara när appen är stängd) | Karantän |
| Kraschdumpar | `%LOCALAPPDATA%\CrashDumps`, WER, `Minidump`, `LiveKernelReports` – äldre än 14 dagar | 0,13 GB | Säker | Karantän |
| Utvecklarcacher | NuGet, npm, pip, cargo, Gradle/Maven, JetBrains-cacher | ~7,7 GB | Byggs upp igen | Verktygets eget kommando (§6.3) |
| Windows Update-rester | `SoftwareDistribution\Download`, CBS-loggar | ~0,8 GB | Säker | Windows Diskrensning (§6.3) |
| Tidigare Windows | `C:\Windows.old` | 30 GB | Försiktig (vägen tillbaka) | Windows (§6.3) |

### 6.2 Visas men erbjuds aldrig

`Windows\Installer`, `Package Cache`, `WinSxS`, Prefetch, pagefile/hiberfil, Store-appars `Packages`, webbläsarprofilers data, OneDrive-platshållare. Var och en med en förklaring varför den inte ska röras.

### 6.3 Åtgärder via Windows eller verktyg

Kategorier som inte rensas med karantän visar en knapp som kör rätt verktyg med tydlig bekräftelse (det går inte att ångra): `cleanmgr` med förvalda kategorier eller Inställningar → Lagring för Windows Update-rester och `Windows.old`; `npm cache clean --force`, `dotnet nuget locals all --clear`, `pip cache purge`, `cargo cache`-motsvarighet (eller borttagning av `registry\cache` via karantän om verktyget saknas) för utvecklarcacher. Byggarens val exakt vilka kommandon, men varje kommando visas i klartext innan det körs.

### 6.4 Förklaring per post

Varje kategori har en mening i klarspråk, t.ex. för `Windows.old`: "Den förra Windows-installationen, sparad vid uppgraderingen 11 oktober. Behövs bara om du vill gå tillbaka. Windows tar bort den själv runt 21 oktober." Datum räknas fram från mappens skapandetid.

## 7. "Vart tog utrymmet vägen?"

Högst en gång per dygn (mitt val, efter granskning), i bakgrunden när programmet startar eller när vyn Utrymme öppnas, sparar Stale en ögonblicksbild av storleken på varje enhet och de största mapparna (två nivåer under rötterna i §4, plus `C:\`-rotens mappar, `Windows.old`, `$Windows.~BT`, Docker/WSL-diskar). En vy visar per enhet: ledigt utrymme då och nu, och de mappar som växt mest sedan förra ögonblicksbilden eller ett valt datum, med förklaring från katalogen när den finns ("+30 GB `Windows.old`, skapad vid Windows-uppgraderingen"). Ögonblicksbilder sparas i `%ProgramData%\Stale Uninstaller\snapshots\` (små JSON, högst 90 sparade, motsvarande ungefär tre månader; äldsta tas bort). Mätningen delar resultat med Mappar-vyn så att samma mapp inte mäts två gånger samma dag.

## 8. Programmens ursprung

Detaljpanelen i programlistan får: utgivare enligt **digital signatur** på huvudprogrammet (signerad av, giltig/ogiltig/osignerad), installationsdatum, **installationskälla** (winget, Microsoft Store, Steam/Epic/GOG/EA/Ubisoft-bibliotek, drivrutinspaket, förinstallerat från tillverkaren, okänt – byggarens heuristik: installationssökväg, registervärden som `InstallSource`, winget-databasen om den går att läsa), och **beskrivning** från huvudprogrammets versionsinformation (`FileDescription`, `CompanyName`).

## 9. Krympa Docker- och WSL-diskar

En egen post i skräpletaren för varje hittad `.vhdx` från WSL-distributioner och Docker Desktop:

1. Visa filstorlek och, om WSL/Docker kör, använt utrymme inuti (`docker system df`, `wsl` + `df`).
2. Valfritt först: `docker system prune` med lista över vad som tas bort. Volymer bara om användaren uttryckligen kryssar i det.
3. Krympa: kontrollera att Docker Desktop är stängt, kör `wsl --shutdown`, sedan `Optimize-VHD -Mode Full` (finns på Pro) eller `diskpart` `compact vdisk`. Erbjud att slå på glest läge (`wsl --manage <distro> --set-sparse true`) där det stöds.
4. Visa storlek före och efter. Filen flyttas eller raderas aldrig. Vägra om WSL eller Docker fortfarande kör.

## 10. Installer och release

- **Inno Setup**-skript i repot (`installer/stale.iss`): installerar `StaleUninstaller.exe` till `%ProgramFiles%\Stale Uninstaller`, Startmeny-genväg, avinstallationspost (Stale registrerar sig själv), kräver administratör. Versionsnummer från `Cargo.toml`.
- **Stales egen avinstallation** (mitt val): avinstalleraren kör `StaleUninstaller.exe --uninstall-cleanup`, som visar en dialog när karantänen har poster: "Karantänen innehåller N GB. Töm den, eller behåll den?" med valen Töm (permanent borttagning av payload och manifest), Behåll (allt lämnas kvar, inklusive `%ProgramData%Stale Uninstaller`), Avbryt avinstallationen. Utan karantänposter tas `%ProgramData%Stale Uninstaller` bort helt.
- **GitHub-release** på `Lonestarrk/stale-uninstaller`: tagg `vX.Y.Z`, setup-filen bifogad, kort release-notis. Okodsignerad i v1; README och release-notisen säger att SmartScreen varnar.
- **Skill** i projektet (`.claude/skills/release/SKILL.md` eller motsvarande namn som byggaren väljer) som beskriver rutinen: bygga release-exe, köra Inno Setup-kompilatorn, versionering, testinstallation, publicera releasen. Dokumentationen i `docs/` förblir verktygsneutral.

## 11. Parkerade fel från plan 2–3

Från backloggen: `resolve_unquoted_path` godtar en katalog som prefix; testkrock kring `SystemRoot`; 8.3-sökvägar i levande mål; kvarvarande filhandtag på planterade inställningsfiler; levande mål som saknas (behållna registernycklar, `ServiceDll`, `rundll32 <dll>`); `shared_folders`/`users_root` läser miljövariabler; borttagning av hel uppgift/CLSID/vendornyckel vid en död referens; platsformatet via `Hive`s Debug; andra profilers Temp; storleksgräns för behåll-listan; svag rendering av andra kontons avinstallerarsökväg. Byggarens val vilka som blir egna uppgifter och vilka som samlas.

## 12. Prestanda

Mappförklararen och ögonblicksbilderna mäter många mappar. Mätningen körs på en begränsad trådpool (4–8 trådar) enligt `docs/research/2026-10-18-parallel-fs-scan.md`, med resultat som strömmar in i vyn medan den mäts. Ingen MFT-läsning i plan 4.

## 13. Gränssnitt

Nya vyer i verktygsraden: **Mappar** (§4–5), **Skräp** (§6, §9) och **Utrymme** (§7). Samma filtrerings-, tabell- och karantänmönster som de befintliga vyerna. All text på svenska i `strings.rs`.

## 14. Överlämnat till byggaren

Katalogens format och innehåll (startar med kategorierna i §6.1 och de vanligaste AppData-mapparna från researchen), exakt heuristik för installationskälla, trådpoolens storlek, hur beroenden upptäcks, exakta verktygskommandon i §6.3, Inno Setup-detaljer, skill-namnet.
