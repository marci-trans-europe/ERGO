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

Az alkalmazás nem küldi el a teljes adatbázissémát minden kérdéssel. A táblák,
oszlopok és idegen kulcsok katalógusát 24 órára helyben gyorsítótárazza, majd a
magyar üzleti kérdés alapján legfeljebb 8 releváns táblát és táblánként 36
fontos oszlopot választ ki. A séma teljes tartalma nem kerül a GitHubra.

Az eredménykártya megmutatja a kiválasztott és teljes séma méretét, valamint az
OpenAI által visszaadott input-, output- és cache-tokenek számát. A második,
összefoglaló AI-hívás legfeljebb 40 eredménysort és 18 000 karaktert kap meg.

Más OpenAI-kompatibilis szolgáltatás használatakor az API-címet és a modell
azonosítóját a kapcsolati beállításokban kell módosítani.

> Az elemzéshez szükséges adatbázis-séma és lekérdezési eredmény az itt
> beállított AI-szolgáltatóhoz kerül feldolgozásra. A választott szolgáltatói
> fióknak meg kell felelnie a vállalati adatkezelési követelményeknek.

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
