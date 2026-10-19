# ERGO · Élő ERP Elemző macOS-re

Az ERGO egy teljesen helyben futó Tauri 2 + Next.js alkalmazás. A céges VPN-en
keresztül közvetlenül, csak olvasási jogosultsággal kapcsolódik a
`treu_replica` MySQL-adatbázishoz, természetes nyelvű kérdésekből biztonságos
lekérdezést készít, majd táblázatban vagy diagramon jeleníti meg az eredményt.

## Pilot architektúra

- nincs Vercel-deploy és nincs külön alkalmazásszerver;
- a kezelőfelület statikus Next.js exportként kerül a natív alkalmazásba;
- a MySQL- és AI-kapcsolat a helyi Rust folyamatban fut;
- a MySQL-jelszó és az AI API-kulcs gépenkénti `.env` fájlból vagy tartalékként
  a macOS Kulcskarikából olvasható;
- a nem titkos beállítások a felhasználó alkalmazáskonfigurációs mappájába
  kerülnek;
- az alkalmazás nem menti le a beszélgetéseket és lekérdezési eredményeket;
- a build ad-hoc aláírást használ (`signingIdentity: "-"`).

## Első indítás

1. Kapcsold be a céges VPN-t.
2. Nyisd meg az ERGO alkalmazást.
3. Hozd létre ezt a fájlt:

   `~/Library/Application Support/hu.transeurope.ergo/.env`

   Tartalma:

   ```dotenv
   MYSQL_PASSWORD=az_adatbazis_jelszava
   AI_API_KEY=az_ai_szolgaltato_api_kulcsa
   ```

4. A fájl jogosultságát érdemes csak a saját felhasználóra korlátozni:

   ```bash
   chmod 600 "$HOME/Library/Application Support/hu.transeurope.ergo/.env"
   ```

5. Az alkalmazás **Kapcsolati beállítások** ablakában szükség esetén módosítsd
   az OpenAI-kompatibilis API-címet és a modellt, majd ellenőrizd a kapcsolatot.

A `.env` sima szöveges fájl, ezért kevésbé biztonságos a Kulcskarikánál. Az
alkalmazás először ezt a fájlt olvassa, és csak akkor használja a Kulcskarikát,
ha a megfelelő változó nincs benne.

Az alapértelmezett AI-beállítás közvetlen OpenAI API:

- API-cím: `https://api.openai.com/v1`
- modell: `gpt-6-astra`

A chat fejlécében lévő modellválasztó az API által elérhetővé tett GPT- és
reasoning modelleket listázza. A választás helyben mentődik, és a következő
indításkor is megmarad.

## Tokenhatékony sémafeldolgozás

Az oldalsáv három gyors elemzése — havi árbevételi trend, top ügyfelek és
lejárt kintlévőségek — előre ellenőrzött, rögzített SQL-lekérdezést használ.
Ezeknél nincs AI-alapú SQL-tervezés vagy sémafeltérképezés: csak az eredmény
szöveges üzleti összefoglalása használ tokent. A szabadon beírt és a
részletező kérdések továbbra is dinamikus lekérdezést készítenek. A
kintlévőségi gyorsnézet a nyitott vevői tételt a számlafejhez és a
számlasorokhoz kapcsolja, ezért számlánként a konkrét cikkeket, mennyiségeket
és nettó sorértékeket is megmutatja.

Az árbevételi gyorsnézetek egységes üzleti definíciót használnak: kizárólag a
Trans Europe Zrt. (`companynumber = '1'`) könyvelt számláit számítják, a
számlafej vállalati alapdevizás nettó értékéből, a jóváírások levonásával. A
dinamikus lekérdezések ellenőrzése ugyanezeket a szabályokat kényszeríti ki;
az áfaösszeg (`vatbase`) nem használható árbevételként.

A vevői vásárlások relációs skillje külön kezeli a számlázott partnert és a
végfelhasználót. Az ilyen kérdéseknél az alkalmazás a `customer`, `endcustomer`,
`invoice` és `invoiceline` kapcsolatát is vizsgálja, majd a jóváírásokkal
korrigált mennyiséget és nettó alapdevizás sorértéket adja vissza. Az OBH
ellenőrzött végfelhasználói aliasaihoz rögzített SQL-terv tartozik, így ennél a
kérdésnél csak a szöveges összefoglaló használ AI-tokeneket.

Az „X végfelhasználó mit vásárolt?” megfogalmazást az ERGO általánosan
felismeri: az `endcustomer` törzsben helyben feloldja a szervezet nevét és
aliasait — többek között a „szoft” és „soft” írásváltozatokat —, majd ugyanazt
az ellenőrzött számla–cikksor relációt használja. A feloldás teljes névelemeket
vizsgál, így például az AERON nem egyezik az AERONAUTICA szórészletre, a közös
névelemmel rendelkező vállalatcsoportok — például a MÁV-változatok — viszont
együtt maradnak. A „top N végfelhasználó” kérdések szintén fix, AI-tervezés
nélküli SQL-lekérdezést használnak, és nettó alapdevizás számlasorérték alapján
rangsorolnak. A hosszú lekérdezési eredmények tördelhető táblázatban jelennek
meg, és egy gombbal teljes képernyős nézetre válthatók.

A vásárlási kérdések magyarul és angolul is megfogalmazhatók. Az alkalmazás a
megnevezett céget a megfelelő törzsben keresi: a „végfelhasználó” az
`endcustomer`, a „viszonteladó” vagy „számlázott vevő” a `customer` felől
kapcsolódik a könyvelt számlákhoz és cikksorokhoz. Ha a szerep vagy a cégnév
hiányzik, az ERGO visszakérdez, mielőtt lekérdezést futtatna. A regressziós
ellenőrzéshez kitölthető, 30 soros munkafüzet található az `evaluation`
mappában.

Az alkalmazás nem küldi el a teljes adatbázissémát minden kérdéssel. A táblák,
oszlopok és idegen kulcsok katalógusát 24 órára helyben gyorsítótárazza, majd a
magyar üzleti kérdés alapján legfeljebb 8 releváns táblát és táblánként 36
fontos oszlopot választ ki. Emellett minden dinamikus kérdés megkapja a
számlázási mag táblákat (`invoice`, `invoiceline`, `customer`, `endcustomer`,
`customerentry`) és — oszlopok nélkül — az összes táblanév listáját, hogy a
modell tudja, mi létezik, és ne találjon ki táblanevet. A séma teljes tartalma
nem kerül a GitHubra.

A sémagyorsítótár a kis kardinalitású kód-, státusz- és dátummezőkhöz tényleges
mintaértékeket is gyűjt, korlátozott olvasási ablakból (legfeljebb 80 oszlop,
oszloponként 10 érték). Ebből derül ki például, hogy a `debitcredit` értéke 0/1,
és hogy egy varchar dátummező valóban ponttal tagolt. A mintavétel legjobb
szándékú: ha nem sikerül, a séma e nélkül is használható.

## Agentikus lekérdezéstervezés

A Responses API-t használó modellek nem egyetlen lövésből terveznek. A
tervezés közben eszközöket hívhatnak, legfeljebb 8 körben:

- `search_schema` és `describe_table` — tábla és oszlop keresése a teljes
  katalógusban, ha a kiválasztott sémarészlet nem elég;
- `resolve_entity` — cégnév feloldása ellenőrzött kódokra, kötelezően megadott
  szereppel (végfelhasználó vagy számlázott vevő);
- `probe_query` — próbalekérdezés legfeljebb 20 sorral és kérdésenként
  legfeljebb 5 alkalommal, a join, a dátumformátum és a kódértékek
  ellenőrzésére;
- `submit_plan` — a végleges terv beadása, ami zárja a ciklust.

A biztonsági modell nem változik: a próbalekérdezések ugyanazon a csak olvasható
őrön futnak, a végleges SQL pedig változatlanul átmegy a teljes biztonsági és
üzleti ellenőrzésen. A chat/completions modellek a korábbi egylövésű úton
maradnak.

A terv ellenőrzése és futtatása egyetlen javító ciklusban történik: ha a
validáció vagy maga a MySQL hibát jelez, a hibaszöveg visszakerül a modellhez,
és legfeljebb két javító kör következik. Kapcsolati és időkorlát-hibánál nincs
újratervezés, mert azt nem a terv okozta.

## ERGO Knowledge Pack és használat közbeni fejlesztés

Az üzleti definíciók, ellenőrzött táblakapcsolatok, entitásaliasok és ismert
elemzési skillek verziózott, helyi Knowledge Packben találhatók. A csomag az
alkalmazás része, nem tartalmaz jelszót vagy tranzakciós adatot. Az AI-val
tervezett lekérdezések csak a kérdéshez szükséges sémarészlet mellett ezt a
kuratált tudást kapják meg; a gyakori skillek továbbra is fix SQL-t használnak.
Az 1.0.1-es tudásverzió a One Magyarország pontos `one` végfelhasználói kódját
elkülöníti az All for One és Euro One szervezetektől; a névcsalád-bővítés csak
külön engedélyezett szabálynál, például a MÁV vállalatcsoportnál történik meg.
Az 1.0.2-es verzió a „4iG által megvásárolt termékek” mondatszerkezetet is fix
vásárlási skillként kezeli, és a 4iG ellenőrzött végfelhasználói kódjait külön
tartja a pusztán hasonló nevű szervezetektől.

Minden elemzői válasz alatt külön jelölhető, hogy helyes volt-e. A javítandó
válaszhoz kategória és szöveges korrekció adható. Az ERGO helyben, az
alkalmazástámogatási mappában lévő `analysis-feedback.json` fájlba menti:

- az eredeti kérdést és az ERGO összefoglalóját;
- a kiválasztott kategóriát és a felhasználó korrekcióját;
- az eredmény címét, sorainak számát, a lekérdezés forrását és SQL-jét;
- az alkalmazás, a modell és az elemzési időablak verzióadatait.

A visszajelzés nem tartalmaz API-kulcsot, adatbázis-jelszót vagy nyers
lekérdezési sorokat, és a mentése nem használ AI-tokeneket. A beállítások
„Tudás és javítások” része mutatja a tudáscsomag állapotát és a fejlesztésre
váró korrekciókat. Ezekből előkészíthető egy e-mailes fejlesztési csomag, de a
visszajelzés biztonsági okból soha nem módosít automatikusan SQL-t vagy üzleti
szabályt. A jóváhagyott javítás a következő Knowledge Pack-verzióval és
regressziós teszttel kerül az alkalmazásba.

Az OpenAI Responses API lekérdezési tervei szigorú JSON-sémát használnak, így
az ERGO csak a várt SQL-, cím-, vizualizáció- és sorszámmezőket fogadja el.

A reasoning modellek a kimeneti tokenkorlátot a belső gondolkodással együtt
költik el, ezért az ERGO explicit reasoning effortot állít be (tervezéshez
közepes, javító körhöz magas, összefoglalóhoz alacsony), és reális kimeneti
plafonnal dolgozik. Ha a válasz mégis a plafon miatt lenne csonka, az ERGO
megemelt kerettel egyszer újrapróbálja, és csak utána jelez hibát — korábban a
csonka válasz észrevétlenül, lapos összefoglalóként jutott a felhasználóhoz.

Az eredménykártya megmutatja a kiválasztott és teljes séma méretét, valamint az
OpenAI által visszaadott input-, output- és cache-tokenek számát. A második,
összefoglaló AI-hívás legfeljebb 40 eredménysort és 18 000 karaktert kap meg,
emellett megkapja a lefuttatott SQL-t, a plan címét, az oszlopneveket, az
időszak feloldását és a beszélgetés utolsó fordulóit. Az összegeket, átlagokat
és szélsőértékeket nem a modell számolja: az ERGO determinisztikusan, a teljes
visszaadott eredményhalmazon számolja ki, és készen adja át. Így a 40 soros
előnézet már nem vezethet téves összesítéshez.

## Elemzési időablak és kifejezetten kért időszak

A fejlécben választott 1, 3 vagy 5 FY-s időablak az alapértelmezés. Ha a kérdés
kifejezetten megnevez egy azon kívüli évet, az a megnevezett időszak
felülírja az ablakot: az ERGO arra az időszakra futtatja a lekérdezést, és az
összefoglalóban ki is mondja. Korábban az ablak akkor is szűkített, ha a
felhasználó tágabb időszakot kért, ezért az app csendben rossz vagy üres
választ adott. Az összefoglaló azt is jelzi, hogy a záró év még nem zárt teljes
év, így a trendösszehasonlítás részleges évet tartalmaz.

Más OpenAI-kompatibilis szolgáltatás használatakor az API-címet és a modell
azonosítóját a kapcsolati beállításokban kell módosítani.

> Az elemzéshez szükséges adatbázis-séma és lekérdezési eredmény az itt
> beállított AI-szolgáltatóhoz kerül feldolgozásra. A választott szolgáltatói
> fióknak meg kell felelnie a vállalati adatkezelési követelményeknek.

## Tanulás a visszajelzésekből

Az ERGO a visszajelzéseket nem csak eltárolja, hanem vissza is olvassa a
promptba. A modell súlyai nem tanulnak; amit az app épít, az **visszakeresett
tapasztalat**: a kérdéshez hasonló korábbi esetek bekerülnek a tervezés
kontextusába.

A hasonlóságot helyben, API-hívás nélkül számolja (Jaccard-hasonlóság a kérdés
kulcsszavain). A magyar ragozást előtagvágás kezeli, így az „árbevétel” és az
„árbevételt” egymásra illeszkedik.

Három réteg, eltérő bizalmi szinttel:

1. **Ellenőrzött lekérdezések.** Amit „helyes”-nek jelölsz és van hozzá SQL, az
   hasonló kérdésnél bekerül a planner promptjába elfogadott példaként,
   legfeljebb négy darab. Ez csak támpont: a végleges SQL ugyanúgy átmegy a
   teljes biztonsági és üzleti ellenőrzésen, ezért egy téves visszajelzés nem
   tud üzleti szabályt felülírni.
2. **Ismert hibák.** A „javítandó” értékeléshez írt korrekció hasonló
   kérdésnél figyelmeztetésként megy vissza. A kifejezetten `summary`
   kategóriába sorolt javítások az összefoglaló promptjába is bekerülnek — ez
   az az eset, amikor az SQL jó volt, de a szöveges válasz félreértelmezte az
   eredményt.
3. **Cégazonosítás, emberi jóváhagyással.** Ha egy kérdésnél több cég is illik
   a megadott névre, az ERGO visszakérdez, és egyúttal jóváhagyásra váró
   bejegyzést vesz fel. A **Beállítások → Tudás és javítások** alatt
   kiválasztható a helyes cég, és onnantól az a név közvetlenül feloldódik,
   visszakérdezés nélkül. Az érvényben lévő azonosítások ugyanitt
   visszavonhatók.

A harmadik réteg szándékosan nem automatikus: az dönti el, melyik cég adatait
összegezzük, tehát közvetlenül befolyásolja a pénzügyi számokat. Jóváhagyás
nélkül a bejegyzés egyetlen lekérdezést sem érint, és a jóváhagyás is csak
olyan ERP-kódot fogad el, amelyet az adatbázis-feloldás tényleg megtalált. A
modell egyik réteget sem írhatja.

A tanult tapasztalat a `learned-aliases.json` és a meglévő
`analysis-feedback.json` fájlban, helyben marad, és nem tartalmaz API-kulcsot,
adatbázis-jelszót vagy nyers lekérdezési sort.

## Biztonsági modell

1. A `TREU` adatbázis-felhasználó csak olvasási jogosultságú.
2. Minden felhasználói lekérdezés `START TRANSACTION READ ONLY` tranzakcióban
   fut.
3. Az alkalmazás csak egyetlen `SELECT` vagy `WITH` utasítást enged.
4. Tiltott az adat- és sémamódosítás, zárolás, fájlművelet, rendszer-séma,
   komment és több SQL-utasítás.
5. Egy lekérdezés legfeljebb 200 sort adhat vissza, és 30 másodperc után leáll.

## Fejlesztés

Előfeltételek: Xcode, Rust és Node.js.

```bash
npm install
npm run desktop:dev
```

## Ellenőrzés és csomagolás

```bash
npm run typecheck
npm run lint
npm run build
source "$HOME/.cargo/env"
cargo test --manifest-path src-tauri/Cargo.toml
npm run desktop:build
```

A kész `.app` és `.dmg` fájl a `src-tauri/target/release/bundle/` mappába kerül.

## Telepítés ad-hoc aláírással

Mivel a pilot nem használ Apple Developer ID-t, a macOS az első indításkor
figyelmeztet. A felhasználó a **Rendszerbeállítások → Adatvédelem és biztonság**
alatt választhatja a **Megnyitás mindenképp** lehetőséget. Ez gépenként egyszeri
lépés. Nyilvános terjesztés előtt Developer ID aláírás és Apple-notarizálás
szükséges.
