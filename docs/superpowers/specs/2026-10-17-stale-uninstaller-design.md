# Stale Uninstaller – designdokument

Datum: 2026-10-17
Status: godkänd 2026-10-17

## 1. Avsikt och varför

Stale Uninstaller är ett Windows-program som hjälper användaren hitta och ta bort
program som inte längre används, och städa bort det som avinstallationen lämnar
efter sig. Förebilden är Revo Uninstaller. Det som skiljer Stale från Revo är att
listan utgår från **när programmet senast användes**, så att det blir lätt att se
vad som bara tar plats. Dessutom visas tydligt **vilken disk** varje program
ligger på.

Namnet: produkten heter *Stale Uninstaller*, körbar fil och kortnamn är `stale`.

Bakgrunden är ett återkommande problem: en dator full av program där man inte
minns vad som används, och mappar i AppData som bara växer utan att man vågar
röra dem.

## 2. Mål, icke-mål och anti-mål

### Mål (version 1)

1. Lista alla klassiska installerade program med namn, utgivare, version,
   storlek, installationsdatum, disk och senast använd.
2. Sortera och filtrera så att oanvända program hamnar överst.
3. Avinstallera ett eller flera köade program i en omgång, med eller utan
   programmets egen avinstallerare.
4. Hitta och ta bort rester i filsystem och register efter avinstallationen.
5. Göra borttagningar ångringsbara via karantän, med möjlighet att tömma
   karantänen direkt när disken är full.

### Senare versioner (inte i version 1, men designen får inte stänga dörren)

- **Version 2: Skräpletare.** Hitta filer som bara tar plats: Windows egna
  tillfälliga nedladdningar, cache- och temp-mappar, och mappar i AppData som
  växer. Kärnan är att *förklara* vad varje mapp är och om den är säker att ta
  bort. Användaren ska kunna märka filer och mappar som "viktiga" så att de
  aldrig visas i sökningen igen. Se avsnitt 9.
- **Version 3: Microsoft Store-appar.**

### Icke-mål

- Store-appar (UWP/MSIX) i version 1.
- Andra operativsystem än Windows. Målet är Windows 10 och 11, 64 bitar.
- Portabla program utan registerpost.
- Drivrutinshantering.
- Nätverk, uppdateringskontroll, telemetri. Programmet kräver ingen internetanslutning.

### Anti-mål (gör programmet till ett misslyckande även om det "fungerar")

- Att användarens egna filer försvinner. Allt som kan vara användardata ska
  vara avbockat och varningsmärkt från start.
- Att gränssnittet fryser under sökningar. All skanning sker i bakgrunden.
- Att något raderas utan väg tillbaka. Standardvägen går alltid via karantän.
- Att "senast använd" visar fel datum med självsäker min. Källan för datumet
  ska alltid gå att se.

## 3. Begränsningar och grundval

Dessa är beslutade i samtalet och ska följas.

| Område | Beslut |
|---|---|
| Språk | Rust |
| Gränssnitt | egui via eframe. Fullt grafiskt fönster, ingen terminalvariant i v1. |
| Plattform | Windows 10/11 x64. Programmet begär administratörsrättigheter vid start (manifest med `requireAdministrator`), eftersom Prefetch, HKLM och tjänster kräver det. |
| Distribution | En enda exe-fil utan externa beroenden. |
| Gränssnittsspråk | Svenska i v1. Alla strängar samlas i en modul så översättning går att lägga till senare. (Mitt val.) |
| Datalagring | JSON-filer under `%ProgramData%\Stale\`. (Mitt val; ProgramData eftersom programmet kör som administratör och data ska gälla hela datorn.) |

## 4. Huvudfönstret

Layout "B" från skisserna: verktygsrad överst, **filterpanel till vänster**,
programtabell i mitten, detaljpanel nederst.

### Verktygsrad

Knappar: *Avinstallera* (markerade), *Tvingad borttagning*, *Sök rester*,
*Uppdatera*, sökfält till höger. Dessutom en flik eller knapp för *Karantän*
(se avsnitt 7).

### Filterpanel

Grupper med antal träffar efter varje rad:

- **Senast använd**: Alla · Över 6 månader · Över 1 år · Aldrig sedd
- **Disk**: en rad per enhetsbokstav som har program
- **Storlek**: Över 1 GB (fler trösklar är byggarens val)

Filter inom en grupp är exklusiva, mellan grupper kombineras de med OCH.

### Tabell

Kolumner: kryssruta · Namn · Utgivare · Disk · Storlek · Installerad · Senast använd.
Standardsortering är *Senast använd*, äldst överst, "Aldrig sedd" allra överst.
Alla kolumner sorterbara. Rader som inte använts på över 6 månader markeras i en
varningsfärg. Disk visas som enhetsbokstav i en liten bricka.

Listan ska klara minst ett par tusen rader utan att hacka.

### Detaljpanel

För markerad rad: fullständig installationsplats, kommandot för avinstallering,
registernyckel, senast använd med **källa** (t.ex. "2026-03-14, Prefetch"),
samt övriga källor om flera finns.

## 5. Datakällor

### Installerade program

Läs registrets Uninstall-nycklar från tre ställen:

- `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`
- `HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall`
- `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall`

Fält: `DisplayName`, `DisplayVersion`, `Publisher`, `InstallLocation`,
`InstallDate`, `UninstallString`, `QuietUninstallString`, `EstimatedSize`,
`DisplayIcon`, `WindowsInstaller`, `SystemComponent`, `ParentKeyName`.
Poster utan `DisplayName`, med `SystemComponent=1`, eller med `ParentKeyName`
(uppdateringar) döljs som standard. En inställning kan visa dem.

**Disk** härleds i ordning: `InstallLocation`, sökvägen i `DisplayIcon`,
sökvägen i `UninstallString`. Saknas alla visas "?".

**Storlek**: `EstimatedSize` om det finns, annars summan av filerna i
`InstallLocation` räknad i bakgrunden efter att listan visats.

### Senast använd

Datumet är det senaste av följande källor. Varje träff sparas med sin källa så
detaljpanelen kan visa den.

1. **Prefetch** – `C:\Windows\Prefetch\*.pf`. Innehåller exe-namn, senaste
   körtid (upp till åtta i nyare format) och refererade sökvägar. Filerna är
   komprimerade med LZXPRESS Huffman på Windows 10/11 och måste packas upp
   (Windows-API:et `RtlDecompressBufferEx` duger). Prefetch kan vara avstängt
   på maskiner med SSD-inställningar eller via policy; då saknas källan tyst.
2. **BAM** (Background Activity Moderator) –
   `HKLM\SYSTEM\CurrentControlSet\Services\bam\State\UserSettings\<SID>\`.
   Fullständig sökväg till exe som värdenamn, FILETIME i värdet. Finns på
   Windows 10 1709 och senare. Enkel och pålitlig.
3. **UserAssist** –
   `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist\{GUID}\Count`.
   Värdenamn ROT13-kodade, senaste körning som FILETIME på byte 60 i det
   72 byte långa värdet. Täcker program startade via Explorer och Startmenyn.
   Läs för alla inloggade användares HKU-grenar, inte bara den aktuella.
4. **Filtider** – senaste ändrings- och åtkomsttid på exe-filer under
   `InstallLocation`. Lägst förtroende; används bara om inget annat finns och
   märks som "osäker" i detaljpanelen.

**Matchning** mot program: en användningspost hör till ett program om exe-filens
sökväg ligger under programmets `InstallLocation`. Prefetch saknar full sökväg i
rubriken men listar refererade filer, som används för matchningen. Saknas
`InstallLocation` matchas på exe-namnet i `DisplayIcon`/`UninstallString`.

**Vad som räknas som "använd"** (beslutat i samtalet): datumet i tabellen är
senaste gången användaren själv startade programmet, inte senaste gången någon
exe i mappen kördes. Annars visar allt med en uppdaterare eller bakgrundstjänst
"använd idag" för evigt. Regeln:

- Ett programs **huvud-exe** är i första hand målet för genvägen i Startmenyn,
  i andra hand filen i `DisplayIcon`, i tredje hand den största exe-filen i
  `InstallLocation`s rot. Flera huvud-exe är tillåtet (t.ex. både 32- och
  64-bitarsvariant).
- Tabellens "Senast använd" = senaste körning av en huvud-exe enligt BAM eller
  Prefetch, **eller** en UserAssist-post för programmet (Explorer-start), vilket
  som är senast.
- Exe-filer som inte är huvud-exe räknas inte in i tabellens datum. Filer vars
  namn innehåller `update`, `updater`, `service`, `helper`, `crash`, `report`,
  `daemon`, `tray` eller som är registrerade som tjänst är alltid uteslutna,
  även om de råkar vara huvud-exe.
- Detaljpanelen visar **alla** exe-filer med körningsposter och deras datum,
  så användaren ser skillnaden mellan "uppdateraren kördes igår" och
  "programmet kördes i mars". Träffar lagras därför per exe, inte bara per
  program.
- Finns körningsposter bara för uteslutna exe-filer visas programmet som
  "Aldrig sedd" i tabellen, med de posterna synliga i detaljpanelen.

Programs utan någon träff visas som **Aldrig sedd**.

### Vilka användare omfattas (mitt val)

Version 1 är byggd för det vanliga hemfallet: personen vid datorn är själv
administratör och kör Stale från sitt eget konto. Då gäller:

- `HKCU` och `%APPDATA%`/`%LOCALAPPDATA%` avser den inloggade användaren som
  startade Stale.
- Användningskällor i registret (BAM, UserAssist) läses för alla hives som är
  laddade under `HKU`, det vill säga alla inloggade användare. Andra profilers
  hives laddas **inte** in i v1.
- Restsökningen i filsystemet går igenom alla profilmappar under `C:\Users`,
  eftersom det bara kräver filrättigheter. Registerrester söks bara i laddade
  hives.
- Program installerade per användare av ett annat konto visas i listan med en
  notis om vilket konto de hör till. Deras avinstallerare körs ändå från
  Stales kontext; misslyckas det hamnar programmet i läget "inte
  avinstallerat" (avsnitt 6.2).

Om Stale startas från ett standardkonto med administratörsuppgifter från ett
annat konto visas en varning vid start om att per-användardata avser
administratörskontot. Stöd för det fallet är inte ett mål i v1.

## 6. Avinstallationsflödet

Användaren bockar i ett eller flera program och trycker *Avinstallera* eller
*Tvingad borttagning*. En sammanfattningsdialog visar kön och två val:

- **Skapa återställningspunkt för hela omgången** (kryssruta, förvalt på).
  Windows nekar en ny punkt om en skapats de senaste 24 timmarna; då visas det
  som information, inte som fel.
- **Kör programmens egna avinstallerare** (förvalt på; av i tvingat läge).

Sedan körs programmen ett i taget:

1. **Återställningspunkt** (en gång, före första programmet).
2. **Egen avinstallerare**: MSI-paket (`WindowsInstaller=1`) körs med
   `msiexec /x {produktkod}`, annars `UninstallString` som den står. Körs
   interaktivt och Stale väntar tills processen avslutats. Hoppas över om
   strängen saknas eller filen inte finns; då faller programmet automatiskt
   till tvingat läge med en notis.
3. **Restsökning** (avsnitt 6.1) för varje program.
4. När hela kön gått igenom visas **en** restvy för alla program, grupperad
   per program, så användaren inte avbryts mellan varje.
5. **Borttagning till karantän** av det som är ibockat.

Stegen visas som en stegindikator (Återställningspunkt · Egen avinstallerare ·
Rester · Klart).

### 6.2 När den egna avinstalleraren avbryts eller misslyckas (mitt val)

Stale väntar på hela processträdet, inte bara den första processen, eftersom
många avinstallerare (t.ex. NSIS) kopierar sig till Temp och avslutar
originalet direkt. När trädet är klart kontrolleras om Uninstall-nyckeln
fortfarande finns **och** `InstallLocation` fortfarande innehåller körbara
filer. Om båda är kvar räknas programmet som **inte avinstallerat**:

- Det får status "Inte avinstallerat" i sammanfattningen, med avslutningskoden.
- Restsökning körs **inte** för det, och det tas inte med i restvyn.
- Användaren erbjuds *Försök igen* eller *Tvingad borttagning* per program.

Det här hindrar att ett program som användaren just avböjde i leverantörens
dialog ändå plockas bort via "Hög"-regeln i restvyn.

### 6.1 Restsökning

Sökningen letar på följande ställen. Varje träff får ett förtroende som styr om
den är ibockad från start.

**Filsystem**

- `InstallLocation` själv, om den finns kvar.
- Mappar vars namn matchar programnamn eller utgivare i: `Program Files`,
  `Program Files (x86)`, `ProgramData`, `%APPDATA%`, `%LOCALAPPDATA%`,
  `%LOCALAPPDATA%\Programs`, `%LOCALAPPDATA%\Temp`, för alla användarprofiler
  under `C:\Users`.
- Genvägar i Startmenyn (alla användare och per användare) och på skrivbordet
  som pekar in i installationsmappen.
- Användarmapparna Dokument, Bilder, Skrivbord och Nedladdningar söks **bara
  på toppnivå** efter mappar vars namn matchar programnamnet. Träffar där får
  alltid förtroende *Låg*: avbockade, med varningstext "troligen dina egna
  filer". Inga filer inuti dessa mappar listas enskilt. (Mitt val: så såg
  skissen i samtalet ut, och det kostar lite att visa men mycket att missa.)

**Register**

- Uninstall-nyckeln själv.
- `HKLM\SOFTWARE`, `HKLM\SOFTWARE\WOW6432Node`, `HKCU\Software` (och andra
  HKU-grenar): nycklar med namn som matchar utgivare och/eller programnamn.
- `Software\Classes`: filtyper, URL-protokoll och CLSID vars standardvärde
  eller `InprocServer32`/`LocalServer32` pekar in i installationsmappen.
- `Run`/`RunOnce`-nycklar (autostart) och `App Paths` som pekar in i
  installationsmappen.

**Övrigt**

- Tjänster vars `ImagePath` pekar in i installationsmappen.
- Schemalagda uppgifter vars åtgärder pekar in i installationsmappen.

**Förtroende**

| Nivå | Regel | Ibockad från start |
|---|---|---|
| Hög | Sökvägen pekar in i installationsmappen, eller nyckeln är Uninstall-nyckeln | Ja |
| Medel | Namnmatchning på program eller utgivare under systemkataloger eller AppData | Ja |
| Låg | Namnmatchning i användarmappar (Dokument, Bilder, Skrivbord, Nedladdningar) eller i delade utgivarnycklar som andra program från samma utgivare använder | Nej, med varningstext |

Knappen *Markera säkra* bockar i hög och medel, bockar ur låg.

Namnmatchning ska vara tålig: skiftlägesokänslig, ignorera versionsnummer och
vanliga suffix (t.ex. "3.6", "(x64)"). Mycket korta eller generiska namn
(färre än fyra tecken, eller ord som "Tools", "Setup") ska inte namnmatchas
alls, bara sökvägsmatchas. Exakta regler är byggarens val, men anti-målet om
användardata gäller.

Restsökning ska också kunna köras fristående via knappen *Sök rester* för ett
program som redan är borta ur listan men lämnat spår. I det läget anges namn
och eventuell mapp manuellt.

### 6.3 Spökprogram: avinstallerade program som lämnat rester

Tillagt 2026-10-17 efter samtal. Ingår i version 1 (plan 2).

Ett program som redan avinstallerats saknar Uninstall-nyckel, så det syns inte
i programlistan. Ändå ligger ofta mappar och registernycklar kvar och tar
plats. Stale ska hitta dem automatiskt, utan att användaren behöver ange namn.

**Spår som avslöjar att ett program funnits, och var:**

1. Registernycklar under `HKLM\SOFTWARE\<Utgivare>\<Program>`,
   `HKLM\SOFTWARE\WOW6432Node\...` och `HKCU\Software\...` som inte matchar
   något installerat program. Värden som `InstallPath`, `InstallDir`,
   `InstallLocation`, `Path` eller liknande ger installationsmappen.
2. Hängande referenser: `Run`/`RunOnce`, `App Paths`, `Software\Classes`
   (CLSID, filtyper, URL-protokoll), tjänster, schemalagda uppgifter och
   genvägar i Startmenyn/skrivbordet vars mål inte längre finns på disk.
   Sökvägen i referensen pekar ut var programmet låg.
3. Körningsspår i Prefetch, BAM och UserAssist för exe-filer som inte längre
   finns. Ger både programnamn, mapp och när det senast kördes.
4. Mappar i `Program Files`, `Program Files (x86)`, `ProgramData`,
   `%APPDATA%`, `%LOCALAPPDATA%` och `%LOCALAPPDATA%\Programs` (för alla
   profiler under `C:\Users`) som inte hör till något installerat program.

**Hur det presenteras.** En egen vy, *Spökprogram*, bredvid programlistan.
Spåren grupperas till en post per troligt program (på mappnamn, nyckelnamn
och exe-namn, normaliserade som i restsökningen). Varje post visar namn,
utgivare om känd, de mappar och nycklar som hittats med storlek, senast
använd från körningsspåren, och en bedömning:

| Bedömning | Regel |
|---|---|
| Troligen avinstallerat | Hängande referenser eller körningsspår för exe som saknas, och ingen matchande Uninstall-nyckel |
| Rester utan installation | Bara mappar/nycklar hittade, inga körningsspår |
| Osäker | Enbart en mapp i AppData med generiskt namn, eller namn som matchar ett installerat program delvis |

Användaren bockar i vad som ska bort; samma förtroendenivåer och samma
karantän som i avsnitt 6.1 och 7 gäller. Mappar i användarens dokumentmappar
visas aldrig här. Poster kan märkas *Behåll*, vilket sparas i samma
viktig-lista som skräpletaren (avsnitt 9) så de inte visas igen.

**Krav på plan 1 från detta:** körningsspår ska sparas även när exe-filen
inte matchar något installerat program, så att spökvyn kan läsa dem utan ny
skanning.

## 7. Karantän och ångra

Inget som Stale tar bort raderas direkt. I stället:

- **Filer och mappar** flyttas till en karantänmapp **på samma volym** som
  filen ligger på: `<enhet>:\Stale\quarantine\<id>\`, med bevarad relativ
  struktur. Då är flytten alltid ögonblicklig, kräver inget extra utrymme och
  frigör rätt disk när posten töms. (Mitt val, ändrat efter granskning: en
  gemensam karantän på C: skulle kopiera gigabyte från D: till C: och
  misslyckas helt om C: är den fulla disken.) Om en volym saknar NTFS eller är
  skrivskyddad kopieras till C: i stället, och det visas i dialogen innan
  borttagningen startar. En flytt som misslyckas (låst fil) rapporteras per
  fil, resten fortsätter.
- **Registernycklar** exporteras till en `.reg`-fil i samma karantänmapp innan
  de raderas.
- **Tjänster och schemalagda uppgifter** stoppas och tas bort; deras
  definition sparas (tjänstens registernyckel som `.reg`, uppgiftens XML).
- En `manifest.json` per karantänpost ligger under
  `%ProgramData%\Stale\quarantine\<id>\` tillsammans med `.reg`-filerna och
  beskriver programnamn, datum, varje flyttad sökväg med ursprung och vilken
  volym den hamnade på, samt total storlek per volym. Karantänvyn visar
  storlek per disk så det syns vad som frigörs var.

**Karantänvyn** listar alla poster med namn, datum, storlek och knapparna
*Ångra* och *Ta bort permanent*. Dessutom *Töm karantän* för allt. Detta är
vägen för att frigöra disk direkt.

**Ångra** flyttar tillbaka filerna, importerar `.reg`-filerna och återskapar
tjänster/uppgifter. Om ursprungsplatsen numera är upptagen rapporteras
konflikten och filen lämnas kvar i karantänen.

**Automatisk tömning**: poster äldre än 30 dagar raderas vid programstart.
Antalet dagar är en inställning, 0 stänger av.

Alla karantänmappar, på varje volym, ska ha rättigheter så att bara
administratörer kan läsa dem, eftersom de kan innehålla annan användares data.

## 8. Arkitektur

Två paket i ett Cargo-arbetsområde:

- **`stale-core`** – all logik utan beroende på egui: registerläsning,
  användningskällor, restsökning, karantän, inställningar. Ska gå att
  enhetstesta utan att röra riktiga registret: åtkomst till register,
  filsystem och Windows-API:er går via abstraktioner som kan bytas mot
  testdubbletter.
- **`stale-gui`** – eframe-programmet. Pratar med kärnan via kommandon och
  tar emot framsteg och resultat via kanaler, så att fönstret aldrig blockeras.
  Alla skanningar och borttagningar körs på bakgrundstrådar.

Rekommenderade beroenden (byggarens val att byta om något bättre finns):
`eframe`/`egui`, `windows` (Windows-API inklusive register, tjänster,
dekomprimering, återställningspunkt via WMI eller `SRSetRestorePoint`),
`serde`/`serde_json`, `chrono` eller `time`.

Exe-filen ska ha en ikon och ett manifest med `requireAdministrator` och
DPI-medvetenhet.

## 9. Version 2: skräpletaren (översikt för framtidssäkring)

Beskrivs här så att v1 inte bygger in hinder. Detaljer brainstormas senare.

- Egen vy bredvid programlistan och karantänen.
- **Katalog över kända platser** med namn, beskrivning i klarspråk, säkerhetsnivå
  (säker att ta bort / tas bort men byggs upp igen / rör ej) och hur den
  rensas. Exempel: `%TEMP%`, `C:\Windows\Temp`,
  `C:\Windows\SoftwareDistribution\Download`, Delivery Optimization-cache,
  webbläsarcacher, kraschdumpar, `%LOCALAPPDATA%`-cacher för kända program.
- **AppData-utforskare**: per mapp i Roaming/Local/LocalLow visas storlek,
  senast ändrad, om ett installerat program matchar mappen (koppling till
  programlistan), och beskrivning om mappen finns i katalogen.
- **Viktig-lista**: användaren märker filer eller mappar som viktiga. Listan
  lagras under `%ProgramData%\Stale\important.json` och allt som ligger under
  en märkt sökväg undantas från alla framtida sökningar. Listan ska gå att se
  och redigera.
- Borttagning går via samma karantän som i v1.

Krav på v1 från detta: karantänen får inte anta att en post hör till ett
program, filterpanelen och tabellen ska vara återanvändbara komponenter, och
inställningar/listor ska ligga i samma lagringsmodul.

## 10. Överlämnat till byggaren

- Exakt trådmodell och kanalutformning mellan kärna och gränssnitt.
- Hur Prefetch-parsningen struktureras och vilka formatversioner som stöds
  (minst version 30, Windows 10/11).
- Exakta regler för namnnormalisering i restsökningen, inom ramarna ovan.
- Fler storlekströsklar i filterpanelen.
- Val mellan WMI och `SRSetRestorePoint` för återställningspunkt.
- Ikon och färgval, inom egui:s standardteman.
- Teststrategi för kärnan. Riktiga Prefetch-filer och registerexporter får
  användas som testfixturer, men inget i testerna får skriva till riktiga
  registret eller radera utanför en temporär mapp.
- Loggning till fil under `%ProgramData%\Stale\logs\` för felsökning.
