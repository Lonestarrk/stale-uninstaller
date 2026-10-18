# Parallell filsystemsskanning och GPU – research (2026-10-18)

Fråga: kan `size.rs`, `fsys.rs` och `leftovers/fs_scan.rs` (idag enkeltrådad rekursiv `read_dir`) göras flertrådade, och kan det GPU-accelereras?
Kort svar: ja till trådar (störst nytta på SSD, måttlig på HDD), nej till GPU. Den verkliga genvägen på NTFS är att läsa MFT direkt.

## 1. Varför katalogvandring är I/O- och syscall-bunden på Windows
- Varje katalog kräver flera syscalls (`FindFirstFileEx`/`NtQueryDirectoryFile`, sedan `FindNextFile` per buffert). Ett separat `metadata()`-anrop per fil kan ge en extra öppning; `WIN32_FIND_DATA` ger redan storlek, så undvik extra anrop där det går.
- Kostnaden domineras av kernel-övergångar, filter drivers (Defender, minifilter) och NTFS-metadataläsning, inte av CPU-arbete i användarkod.
- OS-cachen avgör mycket: en "varm" andra körning är ofta flera gånger snabbare än en kall. Mät alltid kall och varm separat, annars överskattas vinsten.
- SSD/NVMe: djup kö och låg latens, så flera trådar med förfrågningar "i luften" skalar på riktigt (uppskattning: 2–4x innan Defender/kernel-lås blir flaskhals).
- HDD: sökningar dominerar; fler trådar ger mer slumpmässig åtkomst och kan göra det sämre. Håll 1–2 trådar på roterande diskar (kan avgöras via `IOCTL_STORAGE_QUERY_PROPERTY`, seek penalty).
- Slutsats: parallellism hjälper när latens, inte bandbredd, är flaskhalsen, dvs. många små kataloger på SSD.

## 2. Flertrådad vandring i Rust
| Alternativ | Modell | Kommentar |
|---|---|---|
| `jwalk` | rayon, parallellism per katalog, strömmande iterator, sorterad utdata möjlig | Författaren anger ca 4x snabbare än `walkdir` vid sorterad utdata med metadata (egen uppskattning; benchmarktabeller ej verifierade här). Hjälper vid djupa träd med många kataloger, inte en enskild jättekatalog. |
| `ignore::WalkParallel` | egen work-stealing-trådpool (ripgrep) | Callback-baserad, ingen ordning. Full kontroll över filter/avbrott. Gitignore-logiken kan stängas av. Inga siffror hittades i sökningen. |
| Egen `rayon` + `read_dir` | `scope`/`par_bridge` rekursivt | Minst beroenden och full kontroll, men man återuppfinner pool, felhantering och avbrott. |

Trådantal: börja på `min(num_cpus, 8)` för SSD; mer ger sällan något eftersom kernel/Defender blir flaskhals (erfarenhetsbaserat). Använd en begränsad pool (inte global rayon) så UI-tråden inte svälts.
Fallgropar:
- Junctions och andra reparse points: följ inte (`FILE_ATTRIBUTE_REPARSE_POINT`), annars cykler och dubbelräkning.
- Långa sökvägar: prefixa `\\?\` (och `longPathAware` i manifest), annars tappas djupa träd.
- Behörighetsfel (`ERROR_ACCESS_DENIED`, `System Volume Information`): räkna och fortsätt, avbryt inte.
- Ordning: parallell utdata är oordnad; sortera efteråt om resultatet visas.
- Hårdlänkar, sparse och komprimerade filer ger skillnad mellan logisk och allokerad storlek.

## 3. Genvägen: läsa NTFS MFT direkt (WizTree, Everything)
- WizTree läser Master File Table direkt från disk i stället för att gå via katalog-API:t, vilket ger hela diskens storlekar på sekunder. Icke-NTFS-, nätverks- och USB-diskar skannas med en långsammare vanlig metod.
- Krav: administratör + råt volymhandtag (`\\.\C:`). Två vägar:
  1. `DeviceIoControl(FSCTL_ENUM_USN_DATA)`: poster i MFT-ordning med fil-ID, förälder-ID och namn (inte i katalogordning, filerna öppnas inte). Ger *inte* storlek; den kräver extra uppslag (t.ex. `OpenFileById`) eller MFT-parsning. Kräver NTFS 3.0+ eller ReFS.
  2. Parsa `$MFT` själv (`FSCTL_GET_NTFS_VOLUME_DATA` + data runs): ger storlek, allokerad storlek och tider i ett svep.
- Rust-crates:
  - `ntfs` (Colin Finck, v0.4.0): read-only, no_std, safe Rust, NTFS 3.x; man ger själv `Read+Seek` över volymen. Fork `fast_ntfs` har cache.
  - `usn-journal-rs`: USN-journal och MFT-uppräkning som iteratorer, kräver admin, Windows-only. Kontrollera underhållsstatus innan beroende.
  - `usn-parser` (CLI, v0.1.3) och `usn-journal` (v0.1.0, 2020) verkar svagt underhållna.
- Begränsningar: endast NTFS för MFT-parsning (USN-uppräkning även ReFS); inte exFAT/FAT/nätverksdiskar; storlek är logisk (`$DATA`) vs allokerad, välj samma definition som idag; hårdlänkar ger flera namn per post och måste dedupliceras på fil-ID; resident data (små filer i MFT-posten) har allokerad storlek 0; fixup-sekvenser måste tillämpas vid parsning; läsning kringgår ACL, så listan kan innehålla sådant användaren inte kan öppna.
- Passform: Stale Uninstaller körs redan förhöjd, så adminkravet är uppfyllt. Bygg ett `MftIndex` (fil-ID -> förälder, namn, storlek) en gång per volym och besvara alla `dir_size`- och leftover-namnsökningar från minnet. Fallback till walkern för icke-NTFS eller vid fel.

## 4. GPU-acceleration
- Katalogvandring kan inte GPU-accelereras: arbetet är latensbundna syscalls mot kernel och disk. GPU:n kan inte göra syscalls, når inte NTFS-drivrutinen och har ingen dataparallell beräkning att utföra; trädet upptäcks dessutom steg för steg (nästa katalog är okänd tills föregående lästs).
- Där GPU *används* i filrelaterat arbete: bulk-hashning/dedup av stora datamängder, GPU-regex/innehållssökning, och GPUDirect Storage (cuFile) med direkt DMA mellan lagring och GPU-minne. NVIDIA beskriver cuFile som lämpat för grovkorniga strömmande överföringar till buffertar, inte finkornig slumpåtkomst. Kräver CUDA/NVIDIA.
- Verdikt: ingen nytta för den här appen. Datat är metadata (många små poster), CPU-trådar eller MFT-läsning tar redan bort flaskhalsen, och ett CUDA-beroende ger bara distributions- och supportkostnad. Möjlig framtida nisch: hashning av leftover-kandidater, men även det är disk-bundet.

## 5. Rekommendation (rangordnad)
1. **Snabb vinst: parallellisera per-program `dir_size`** med en begränsad trådpool (rayon `ThreadPoolBuilder`, 4–8 trådar). Oberoende jobb, liten kodändring, ingen ny semantik. Förväntat ca 2–4x på SSD vid många program, litet på HDD, inget för ett enda stort program.
2. **Medelstor ändring: byt walker i leftover-skanningen (flera profiler/rötter) till `jwalk` eller `ignore::WalkParallel`**, med explicit junction-skydd, `\\?\`-sökvägar och felräkning, plus avbrytbar token. Förväntat 2–4x (jwalk-författarens ~4x är bästa fall); varm cache minskar skillnaden.
3. **Större ändring: valfri MFT-snabbväg bakom feature flag** (t.ex. `mft-fastpath`) för NTFS-volymer när processen är förhöjd, med automatisk fallback till walkern (icke-NTFS, fel, ej admin). Förväntat sekunder för hela volymen i stället för tiotals sekunder till minuter, men värt det först om många kataloger mäts; kräver tester för hårdlänkar, allokerad vs logisk storlek och sparse-filer. Starta med `ntfs` eller `usn-journal-rs` efter underhållskontroll.
4. GPU: gör inget.

Ärligt om vinsterna: siffrorna är uppskattningar från källor och erfarenhet, inte mätningar på vår kod. Gör en liten benchmark (kall/varm cache, SSD/HDD) innan steg 2 och 3.

## Källor
https://docs.rs/crate/jwalk/latest
https://github.com/Byron/jwalk
https://users.rust-lang.org/t/lib-jwalk-fast-recursive-directory-walk/25126
https://github.com/burntsushi/ripgrep
https://diskanalyzer.com/about
https://techspot.com/downloads/7470-wiztree.html
https://docs.rs/usn-journal-rs
https://learn.microsoft.com/en-us/windows/win32/fileio/walking-a-buffer-of-change-journal-records
https://rust-digger.code-maven.com/crates/usn-parser
https://rust-digger.code-maven.com/crates/usn-journal
https://docs.rs/crate/ntfs/latest
https://docs.rs/crate/fast_ntfs/1.0.1
https://colinfinck.de/posts/an-implementation-of-the-ntfs-filesystem-in-a-rust-crate/
https://docs.nvidia.com/gpudirect-storage/overview-guide/
