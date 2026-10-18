# Katalog över skräpfiler för Stales skräpletare (Plan 4)

Datum: 2026-10-18. Underlag för "skräpletare". Storlekar markerade **(mätt)** är uppmätta på utvecklarens egen dator (Windows 11 Pro, spel + utveckling, icke-förhöjd, skrivskyddat). Övriga storlekar är typiska värden från källorna och bör läsas som grov uppskattning. Många källor är sekundära (bloggar, städverktygsleverantörer); där officiell Microsoft-dokumentation saknas står det angivet.

## 1. Kort sammanfattning

Skräp på en Windows-dator delas in i sex klasser:

1. **Tillfälligt och loggar** (`%TEMP%`, `Windows\Temp`, CBS-loggar). Mätt: `%TEMP%` 5,7 GB (varav 3,9 GB är undermappen `Temp\claude`, ett utvecklingsverktygs arbetsyta), CBS-loggar 0,7 GB.
2. **Windows Update och uppgraderingsrester** (`SoftwareDistribution\Download`, `Windows.old`, `$Windows.~BT`, Delivery Optimization). Mätt: `Windows.old` **30 GB**, `SoftwareDistribution\Download` 0,1 GB.
3. **Kraschdumpar och felrapporter** (WER, `LiveKernelReports`, `Minidump`, `CrashDumps`). Mätt: `CrashDumps` 0,13 GB.
4. **Cacher som byggs upp igen** (miniatyrer, shader-cache, webbläsar- och Electron-cache, launcher-cache). Det typiska "växer och växer"-området i AppData. Mätt: AMD DxCache 1,2 GB, NVIDIA DXCache 0,6 GB, D3DSCache 0,16 GB, Chrome 0,22 GB, Edge 0,12 GB, Discord 0,19 GB, VS Code 0,17 GB, Steam htmlcache 0,4 GB.
5. **Utvecklarverktygens cacher** (npm, NuGet, cargo, Gradle, Docker/WSL). Störst på en utvecklardator. Mätt: Docker-data **24 GB**, NuGet 3,3 GB, npm 2,4 GB, JetBrains 1,4 GB, cargo registry 0,55 GB, pip 0,03 GB.
6. **Installationsrester** (`ProgramData\Package Cache`). Mätt 4,1 GB, men detta är *inte* fritt skräp (se 2.4).

Dessutom finns saker som **ser ut som skräp men inte är det** (2.10), t.ex. `Windows\Installer` (mätt 7,5 GB), Prefetch, WinSxS, pagefile/hiberfil och webbläsarprofiler.

Realistisk riskfri vinst på denna typ av dator: ca 10-15 GB (Temp, shader-cache, webbläsar-/Electron-cache, utvecklingscacher), plus Windows.old (30 GB) och Docker-disken med mer ansträngning.

## 2. Kategorier

Säkerhetsskala: **Säker** (rent skräp), **Byggs upp igen** (kan raderas utan skada men kostar tid/nedladdning), **Försiktig** (kan ha värde; kräver förklaring/bekräftelse), **Rör ej**.

### 2.1 Windows temp och loggar

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%TEMP%` (`%LOCALAPPDATA%\Temp`) | Tillfälliga filer från program och installerare | 0,5-10 GB (mätt 5,7 GB) | Säker för filer äldre än ~7 dagar och som inte används; nyare kan tillhöra körande program | Ta bort innehållet (inte mappen); hoppa över låsta filer | [Storage Sense](https://support.microsoft.com/kb/5000679) |
| `%WINDIR%\Temp` | Systemets tillfälliga filer | 0-2 GB (mätt 0) | Säker (äldre än 7 dagar); kräver admin | Ta bort innehållet | Storage Sense |
| `%WINDIR%\Logs\CBS` | Loggar från komponentservicing | 0,1-2 GB (mätt 0,7 GB) | Säker för gamla `*.log`/`*.cab`; behövs bara vid felsökning av Windows Update | Ta bort filer äldre än 30 dagar; admin | [winhelponline](https://www.winhelponline.com/blog/cleanup-component-store-winsxs/) (sekundär) |
| `%LOCALAPPDATA%\Microsoft\Windows\INetCache` | IE/Edge-legacy-cache | liten (mätt 0) | Byggs upp igen | Ta bort innehåll | BleachBit |
| Papperskorgen `$Recycle.Bin` | Raderade filer | varierar | Försiktig: användarens egna filer. Storage Sense tömmer efter vald tid (standardvärdet kunde ej bekräftas) | Windows API `SHEmptyRecycleBin`, inte manuell radering | [Storage Sense](https://support.microsoft.com/kb/5000679) |

### 2.2 Windows Update och servicing

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%WINDIR%\SoftwareDistribution\Download` | Nedladdade uppdateringspaket | 0,1-5 GB (mätt 0,11 GB) | Byggs upp igen; pågående uppdatering kan avbrytas, stoppa `wuauserv` först | Disk Cleanup "Windows Update Cleanup" eller stoppa tjänst och töm mappen | [ControlUp](https://www.controlup.com/script-library-posts/clean-windows-system-drive/) (sekundär) |
| `%WINDIR%\WinSxS` | Komponentlagret, mest hårdlänkar | 5-15 GB rapporterat | **Rör ej manuellt.** Rensa endast via DISM | `DISM /Online /Cleanup-Image /StartComponentCleanup` (tar bort ersatta versioner utan 30-dagarsväntan). `/ResetBase` gör att installerade uppdateringar inte kan avinstalleras; kör aldrig automatiskt | [Microsoft Learn](https://learn.microsoft.com/en-au/windows-hardware/manufacture/desktop/clean-up-the-winsxs-folder?view=windows-11) |
| `C:\Windows.old` | Föregående Windows-installation | 10-30 GB (mätt 30 GB) | Försiktig: raderingen tar bort möjligheten att gå tillbaka. Microsoft: tas bort automatiskt efter 10 dagar; en blogg påstår 30 dagar vid uppgradering från Win 7/8 (obekräftat, källorna är oense) | Disk Cleanup "Previous Windows installation(s)" / Inställningar > Lagring. Inte manuellt (ägarskap/ACL) | [Microsoft Support](https://support.microsoft.com/en-us/help/17125/windows-8-restore-files-old-folder-upgrade) |
| `C:\$Windows.~BT`, `C:\$Windows.~WS` | Rester av uppgraderingsstaging | 5-20 GB | Försiktig; säker när ingen uppgradering pågår | Disk Cleanup "Temporary Windows installation files" | [Wikipedia](https://en.wikipedia.org/wiki/Disk_Cleanup) |
| Delivery Optimization-cache (`%WINDIR%\ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache`) | Uppdateringsfiler som delas med andra datorer | 0-10 GB (mätt: åtkomst nekad utan admin) | Byggs upp igen | Disk Cleanup "Delivery Optimization Files" eller Inställningar > Leveransoptimering. Ta inte bort mappen manuellt | [Kudu](https://usekudu.com/guides/cleaning/windows-delivery-optimization-files-remove) (sekundär) |
| Device Driver Packages | Äldre drivrutinsversioner som reserv | 0,5-5 GB | Försiktig: utan dem går det inte att rulla tillbaka drivrutin | Disk Cleanup-hanteraren "Device Driver Packages" | Wikipedia |

### 2.3 Kraschdumpar och felrapporter

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%ProgramData%\Microsoft\Windows\WER\ReportQueue` och `ReportArchive` | Köade och arkiverade felrapporter | 0 till flera GB (mätt 0) | Säker (äldre än 30 dagar) | Ta bort undermappar äldre än 30 dagar | [WOSHub](https://woshub.com/wer-windows-error-reporting-clear-reportqueue-folder-windows/) |
| `%LOCALAPPDATA%\Microsoft\Windows\WER` | Användarens felrapporter | liten | Säker | Ta bort innehållet | WOSHub |
| `%LOCALAPPDATA%\CrashDumps` | Programkraschdumpar (`*.dmp`) | 0,1-5 GB (mätt 0,13 GB) | Säker när man inte felsöker | Ta bort `*.dmp` äldre än 14 dagar | Kudu/BleachBit (sekundär) |
| `%WINDIR%\Minidump`, `%WINDIR%\MEMORY.DMP` | Dumpar efter blåskärm | 0,1-3 GB (mätt 0) | Säker när orsaken är utredd | Ta bort; admin | [Tech Community](https://techcommunity.microsoft.com/discussions/windowsinsiderprogram/is-it-safe-to-delete-livekernelreports-watchdog-dmp-files/4213915) |
| `%WINDIR%\LiveKernelReports` | Watchdog-/kernel-dumpar | 0-10 GB (mätt 0) | Säker när man inte felsöker (forum, ingen officiell källa) | Ta bort `*.dmp`; admin | Tech Community, [ElevenForum](https://www.elevenforum.com/t/can-i-delete-livekernelreports-watchdog-dmp.5097/latest) |

### 2.4 Installer-cacher

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%ProgramData%\Package Cache` | Bootstrapper-cache (Visual Studio, .NET, VC++ m.fl.) som behövs för reparera/avinstallera | 1-10 GB (mätt 4,1 GB) | **Försiktig / visa bara.** Radering gör att reparation/avinstallation kan kräva originalmedia | Avinstallera programmet (rensar sin del) | Ingen officiell Microsoft-sida hittad |
| `%WINDIR%\Installer\$PatchCache$` | Cache för MSI-lagningar | 0 till flera GB | Försiktig: Windows Installer behöver den | Rensa ej manuellt | [winhelponline](https://www.winhelponline.com/blog/windows-installer-folder-safe-cleanup-free-disk-space/) |
| `%WINDIR%\Downloaded Installations` | Installationsfiler nedladdade av Windows Installer | liten | Försiktig | Rör ej i v1 | Ingen stark källa |
| `%USERPROFILE%\Downloads` (installerare, ISO, ZIP) | Användarens egna filer | varierar (mätt 1 MB) | Försiktig: aldrig automatiskt; bara förslag utifrån ålder/filtyp | Karantän efter uttrycklig bekräftelse | [Storage Sense](https://support.microsoft.com/kb/5000679) har en valfri Downloads-regel |

### 2.5 Cacher som byggs upp igen

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%LOCALAPPDATA%\Microsoft\Windows\Explorer\thumbcache_*.db`, `iconcache_*.db` | Miniatyr- och ikoncache | 20-500 MB (mätt 20 MB) | Byggs upp igen; låsta av explorer.exe | Disk Cleanup "Thumbnails" | Wikipedia |
| `%LOCALAPPDATA%\D3DSCache` | DirectX shader-cache | 0,1-2 GB (mätt 0,16 GB) | Byggs upp igen (första spelstart långsammare) | Ta bort innehåll | [Pax Dei support](https://support.playpaxdei.com/hc/articles/36792360503581/) (svag) |
| `%LOCALAPPDATA%\NVIDIA\DXCache`, `GLCache` | NVIDIA shader-cache | 0,1-5 GB (mätt 0,6 + 0,01 GB) | Byggs upp igen | Ta bort innehåll när spel inte körs | [Gaijin](https://support.gaijin.net/hc/en-us/articles/30268449678994-How-to-clear-NVIDIA-and-DirectX-cache), [Kudu](https://usekudu.com/cleaners/nvidia-cache) |
| `%LOCALAPPDATA%\AMD\DxCache` (även `DxcCache`, `GLCache`) | AMD shader-cache | 0,1-5 GB (mätt 1,2 GB) | Byggs upp igen | Ta bort innehåll | [Open co-op](https://discourse.weareopen.coop/news/amd-dxcache-is-it-safe) (svag) |
| Steam `steamapps\shadercache\<appid>` | Per-spel shader-cache | 0,5-10 GB | Byggs upp igen. Ingen källa hittad i sökningen | Per spel-mapp; Steam stängt | - |

### 2.6 Webbläsare (endast cache)

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%LOCALAPPDATA%\Google\Chrome\User Data\<profil>\Cache`, `Code Cache`, `GPUCache` | Chromium-cache | 0,2-3 GB (mätt 0,22 GB) | Byggs upp igen; stäng webbläsaren | Ta bort innehåll | [BleachBit google_chrome.xml](https://github.com/bleachbit/bleachbit/blob/master/cleaners/google_chrome.xml) |
| `%LOCALAPPDATA%\Microsoft\Edge\User Data\<profil>\Cache`, `Code Cache`, `GPUCache` | Samma i Edge | 0,1-2 GB (mätt 0,12 GB) | Byggs upp igen | Som ovan | BleachBit (samma modell) |
| `%LOCALAPPDATA%\Mozilla\Firefox\Profiles\*\cache2` | Firefox-cache | 0,1-2 GB | Byggs upp igen | Ta bort innehåll | BleachBit (ej verifierad här) |
| `Cookies`, `Login Data`, `History`, `Bookmarks`, `Web Data`, `Local Storage`, `IndexedDB`, `Extensions` | Användarens profildata | - | **Rör ej** (BleachBit håller dem i egna varnade val) | - | BleachBit google_chrome.xml |

### 2.7 Electron-appar (Discord, Teams, Slack, VS Code m.fl.)

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%APPDATA%\<app>\Cache` (nyare `Cache\Cache_Data`), `Code Cache`, `GPUCache`, `DawnCache`, `CachedData` | Chromium-HTTP-cache, kompilerad JS, GPU-data | 0,1-1 GB per app (mätt: Discord 0,19, VS Code 0,17 GB) | Byggs upp igen; appen måste vara stängd. Gammal Code Cache efter uppdatering kan ge fel | Ta bort innehåll | [Kudu](https://usekudu.com/cleaners/todoist) (leverantörssida, svag) |
| `%APPDATA%\<app>\Service Worker\CacheStorage` | Offline-innehåll för webbappar | varierar | Försiktig; ej verifierad, ta inte med i v1 | - | Ingen stark källa |
| `Local Storage`, `IndexedDB`, `Session Storage`, `Cookies` | Inloggning och apptillstånd | - | **Rör ej** | - | - |
| `%APPDATA%\Code\User`, `%USERPROFILE%\.vscode\extensions` | Inställningar och tillägg | - | **Rör ej** | - | - |

Nya Teams (MSIX) ligger under `%LOCALAPPDATA%\Packages\MSTeams_*` (se 2.10).

### 2.8 Spelstartare och drivrutiner

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%LOCALAPPDATA%\Steam\htmlcache` | Steams inbyggda webbläsarcache | 0,1-1 GB (mätt 0,39 GB) | Byggs upp igen; Steam stängt | Ta bort innehåll | Ingen stark källa |
| `%LOCALAPPDATA%\EpicGamesLauncher\Saved\webcache*` | Epics webbcache | 0,1-1 GB | Byggs upp igen | Ta bort innehåll | Ej verifierad |
| NVIDIA/AMD shader-cache | Se 2.5 | | | | |
| Spelmappar, `Saved Games`, `Documents\My Games` | Spel och sparfiler | - | **Rör ej** | - | - |

### 2.9 Utvecklarverktyg

| Plats | Vad det är | Typisk storlek | Säkerhet | Hur det rensas | Källa |
|---|---|---|---|---|---|
| `%LOCALAPPDATA%\npm-cache` | npm-cache (Windows; `npm config get cache`) | 0,5-5 GB (mätt 2,4 GB) | Byggs upp igen (självläkande) | `npm cache clean --force` | [Mole](https://mole.fit/blog/how-to-clear-dev-caches-mac) (sekundär) |
| `%USERPROFILE%\.nuget\packages` | NuGets globala paketmapp | 1-10 GB (mätt 3,3 GB) | Byggs upp igen via restore; offline-bygge fungerar inte tillfälligt | `dotnet nuget locals all --clear` | [Microsoft Learn](https://learn.microsoft.com/en-us/NuGet/reference/cli-reference/cli-ref-locals) |
| `%LOCALAPPDATA%\pip\Cache` | pip-cache | 0-2 GB (mätt 0,03 GB) | Byggs upp igen | `pip cache purge` | Mole |
| `%USERPROFILE%\.cargo\registry\cache`, `registry\src`, `.cargo\git` | cargo-nedladdningar | 0,5-10 GB (mätt `registry` 0,55 GB) | Byggs upp igen. **Radera aldrig hela `.cargo`** (`bin`, config, inloggning) | Ta bort cache/src | Mole |
| `%USERPROFILE%\.rustup\toolchains` | Installerade verktygskedjor | 1-5 GB (mätt 1,3 GB) | **Rör ej** (användarens verktyg) | `rustup toolchain uninstall` | - |
| `%USERPROFILE%\.gradle\caches`, `.m2\repository` | Gradle/Maven | 1-20 GB (finns ej på maskinen) | Byggs upp igen | Stoppa daemon, ta bort | Ingen verifierad källa |
| `%LOCALAPPDATA%\JetBrains\<produkt><ver>` | IDE-cache, index, loggar | 0,5-5 GB (mätt 1,4 GB) | Byggs upp igen (omindexering); stäng IDE | Ta bort / "Invalidate Caches" | Ingen stark källa |
| `%LOCALAPPDATA%\Docker\wsl\...\ext4.vhdx` (nyare `docker_data.vhdx`) | Dockers/WSL2:s virtuella disk; växer men krymper aldrig själv | 5-300 GB (mätt Docker-mapp 24 GB) | **Försiktig**: images, volymer, databaser. Aldrig radera filen | Rensa i Docker (`docker system prune`), sedan `wsl --shutdown` + `Optimize-VHD -Mode Full` (Pro) eller `diskpart` compact. Resultat varierar kraftigt (377 till 117 GB hos en, nästan inget hos en annan) | [Hanselman](https://www.hanselman.com/blog/shrink-your-wsl2-virtual-disks-and-docker-images-and-reclaim-disk-space), [Janetakis](https://nickjanetakis.com/blog/reclaiming-tons-of-diskspace-by-compacting-your-docker-desktop-wsl-2-vm) |
| `target\`, `node_modules`, `bin`/`obj` i projekt | Bygg-utdata | 0,1-20 GB per projekt | Byggs upp igen, projektspecifikt | Utanför v1 | - |

### 2.10 Saker som SER UT som skräp men inte är det

| Plats | Varför det inte är skräp | Säkerhet | Källa |
|---|---|---|---|
| `%WINDIR%\Installer` (mätt 7,5 GB) | `.msi`/`.msp` som Windows Installer behöver för uppdatering, reparation och avinstallation. Att tömma den kan göra program ooinstallerbara. Orphan-verktyg (PatchCleaner) ger falska positiva (t.ex. Adobe Reader) och rekommenderar att *flytta* i stället för radera | **Rör ej** | [PatchCleaner](https://www.homedev.com.au/Free/PatchCleaner), [winhelponline](https://www.winhelponline.com/blog/windows-installer-folder-safe-cleanup-free-disk-space/) |
| `%WINDIR%\Prefetch` | Hjälper Windows starta program snabbare; byggs om, ingen vinst med radering (mätt: 0 läsbart utan admin) | **Rör ej** | Allmän kunskap; ingen stark källa |
| `%WINDIR%\WinSxS` | Hårdlänkar; storleken i Utforskaren är missvisande; manuell radering skadar Windows | **Rör ej** (endast DISM) | [Microsoft Learn](https://learn.microsoft.com/en-au/windows-hardware/manufacture/desktop/clean-up-the-winsxs-folder?view=windows-11) |
| `pagefile.sys`, `hiberfil.sys`, `swapfile.sys` | Virtuellt minne och viloläge/snabbstart; styrs via systeminställningar (`powercfg /h off`), inte filradering | **Rör ej** | Ingen källa citerad |
| Webbläsarprofil: cookies, lösenord, historik, tillägg | Användarens data och inloggningar | **Rör ej** | BleachBit |
| `%LOCALAPPDATA%\Packages\<PackageFamily>` (mätt 2,0 GB) | Butiksappars data och inställningar (`LocalState`). Endast `TempState`/`LocalCache` är cache, och bara med app-specifik kunskap | **Rör ej** i v1 | Ingen officiell källa hittad |
| OneDrive-platshållare (`%USERPROFILE%\OneDrive`) | Molnfiler visas som platshållare; radering kan ta bort filen i molnet | **Rör ej** | Allmän kunskap |
| Återställningspunkter (`System Volume Information`) | Hanteras via Systemskydd/`vssadmin`, inte filsystemet | **Rör ej** (visa som info) | - |

## 3. Hur andra verktyg avgör säkerhet

- **BleachBit CleanerML**: varje program är en `<cleaner>` med `<option>`-poster (Cache, Cookies, History, Passwords ...). Varje option har `<action command="delete" search="file|glob|walk.files|walk.all" path="...">` och kan ha en `<warning>` ("This option will delete your saved passwords"). Cache och personlig data hålls alltid i skilda options. En `<running>`-kontroll vägrar köra när programmet är aktivt. Variabler som `$$profile$$` och `$$base$$` (= `%LocalAppData%\Google\Chrome\User Data`) gör definitionerna återanvändbara. Källa: [google_chrome.xml](https://github.com/bleachbit/bleachbit/blob/master/cleaners/google_chrome.xml).
- **Disk Cleanup (cleanmgr)**: varje kategori är en hanterare under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\VolumeCaches\<namn>`. `cleanmgr /sageset:N` sparar valen som `StateFlagsNNNN` och `/sagerun:N` kör dem. Vilka hanterare som finns beror på Windows-version. Källa: ControlUp och Wikipedia; ingen fullständig officiell Microsoft-lista hittades (mitt försök att hämta Microsoft Learn-sidan om VolumeCaches gav 404).
- **Storage Sense**: regelbaserad. Tar bort temporära filer som appar inte använder, tömmer Papperskorgen efter vald tid och kan (valfritt) rensa Downloads som inte öppnats på N dagar. Verkar som standard bara på systemdisken. De exakta standardvärdena för ålder kunde inte bekräftas. Källa: [Microsoft Support](https://support.microsoft.com/kb/5000679).
- **DISM**: `/StartComponentCleanup` raderar ersatta komponentversioner direkt, utan den schemalagda uppgiftens 30-dagarsfördröjning.
- **PatchCleaner**: orphan-detektion mot MSI-registret, med uteslutningsfilter och "flytta, radera inte". Visar att även specialverktyg gör fel.

Oenighet mellan källor: Windows.old-fristen (10 dagar enligt Microsoft, 30 enligt en blogg); LiveKernelReports (forum: säker, men spara dumpar vid pågående kraschutredning).

## 4. Rekommendation för Stale v1

Rangordnat efter vinst x säkerhet (mätt storlek på utvecklarens dator):

1. **%TEMP% (äldre än 7 dagar, ej i bruk)**, 5,7 GB. Säker.
2. **Shader-cache** (AMD DxCache, NVIDIA DXCache/GLCache, D3DSCache), ca 2,0 GB. Byggs upp igen.
3. **Webbläsar- och Electron-cache** (Chrome, Edge, Discord, VS Code, Steam htmlcache), ca 1,3 GB. Endast när appen inte körs.
4. **Utvecklingscacher** (NuGet 3,3, npm 2,4, JetBrains 1,4, cargo 0,55, pip 0,03 GB), ca 7,7 GB. Använd verktygets eget kommando när det finns.
5. **Windows.old** (30 GB) och `$Windows.~BT`. Försiktig: starta Windows egen rensning (`cleanmgr`) i stället för att radera själv.
6. **Kraschdumpar** (CrashDumps 0,13 GB, WER, Minidump, LiveKernelReports). Säker efter 14 dagar.
7. **CBS-loggar** (0,7 GB) och `SoftwareDistribution\Download` (0,1 GB). Admin; Disk Cleanup "Windows Update Cleanup".
8. **Docker/WSL-disk** (24 GB). Endast information och hänvisning till komprimering; aldrig flytta filen.

Bara visa (ingen radering): Package Cache (4,1 GB), Downloads-installerare, Delivery Optimization, `Packages`.

**Detekteringsregler**
- Ålder: Temp äldre än 7 dagar, dumpar 14 dagar, WER/CBS 30 dagar (senast ändrad). Hela app-cachemappar får tas oavsett ålder om appen inte körs.
- I bruk: hoppa över filer som inte går att öppna exklusivt; vägra rensa en app-cache om dess process körs (som BleachBit `<running>`).
- Storlek: visa endast poster över ca 50 MB totalt.
- Allt via Stales befintliga karantän (ångra möjligt). Undantag: poster som bara Windows egna verktyg (DISM, Disk Cleanup, Delivery Optimization) kan hantera; markera "kan inte ångras" och starta verktyget.
- Följ inte junctions/symlänkar; arbeta endast under vitlistade rotprefix. Admin-poster markeras separat.

**Rör aldrig**: `Windows\Installer`, WinSxS (manuellt), Prefetch, pagefile/hiberfil, webbläsarprofildata, `Local Storage`/`IndexedDB`, `Packages\*\LocalState`, OneDrive-platshållare, spelsparningar, `.cargo\bin`, `.rustup`, Docker-diskavbildningen, återställningspunkter, Package Cache.

**En mening per post (förslag)**
- Temp: "Tillfälliga filer som program lämnat kvar; de behövs inte när programmet är stängt."
- Shader-cache: "Grafikdrivrutinens sparade spelberäkningar; byggs upp igen, så nästa spelstart kan bli lite långsammare."
- Webbläsarcache: "Sparade webbsidebilder som gör sidor snabbare; dina inloggningar och lösenord påverkas inte."
- Electron-cache: "Programmets egna kopior av webbinnehåll; hämtas om vid nästa start."
- NuGet/npm/cargo: "Nedladdade paket som hämtas igen vid nästa bygge; första bygget blir långsammare."
- Kraschdumpar: "Minnesavtryck från krascher, bra för felsökning men annars onödiga."
- Windows.old: "Din gamla Windows-version; tar du bort den kan du inte gå tillbaka efter uppgraderingen."
- Docker-disk: "Dockers virtuella disk; växer men krymper inte av sig själv, kan komprimeras i Docker."

## 5. Källor

1. https://support.microsoft.com/kb/5000679 (Storage Sense)
2. https://learn.microsoft.com/en-au/windows-hardware/manufacture/desktop/clean-up-the-winsxs-folder?view=windows-11
3. https://support.microsoft.com/en-us/help/17125/windows-8-restore-files-old-folder-upgrade
4. https://learn.microsoft.com/en-us/NuGet/reference/cli-reference/cli-ref-locals
5. https://github.com/bleachbit/bleachbit/blob/master/cleaners/google_chrome.xml
6. https://en.wikipedia.org/wiki/Disk_Cleanup
7. https://www.controlup.com/script-library-posts/clean-windows-system-drive/
8. https://www.homedev.com.au/Free/PatchCleaner
9. https://www.winhelponline.com/blog/windows-installer-folder-safe-cleanup-free-disk-space/
10. https://www.winhelponline.com/blog/cleanup-component-store-winsxs/
11. https://woshub.com/wer-windows-error-reporting-clear-reportqueue-folder-windows/
12. https://techcommunity.microsoft.com/discussions/windowsinsiderprogram/is-it-safe-to-delete-livekernelreports-watchdog-dmp-files/4213915
13. https://www.elevenforum.com/t/can-i-delete-livekernelreports-watchdog-dmp.5097/latest
14. https://usekudu.com/guides/cleaning/windows-delivery-optimization-files-remove
15. https://support.gaijin.net/hc/en-us/articles/30268449678994-How-to-clear-NVIDIA-and-DirectX-cache
16. https://support.playpaxdei.com/hc/articles/36792360503581/
17. https://usekudu.com/cleaners/nvidia-cache
18. https://usekudu.com/cleaners/todoist
19. https://discourse.weareopen.coop/news/amd-dxcache-is-it-safe
20. https://www.hanselman.com/blog/shrink-your-wsl2-virtual-disks-and-docker-images-and-reclaim-disk-space
21. https://nickjanetakis.com/blog/reclaiming-tons-of-diskspace-by-compacting-your-docker-desktop-wsl-2-vm
22. https://mole.fit/blog/how-to-clear-dev-caches-mac

Luckor (ingen stark källa): Steam shadercache, Epic webcache, Gradle/Maven, JetBrains, `Packages\TempState`, Prefetch, `Service Worker\CacheStorage`, officiell lista över VolumeCaches-hanterare, Package Cache. Verifiera innan de tas med i regelverket.
