import en from "./locales/en.json";
import de from "./locales/de.json";
import fr from "./locales/fr.json";
import es from "./locales/es.json";
import pl from "./locales/pl.json";
import it from "./locales/it.json";
import nl from "./locales/nl.json";
import cz from "./locales/cz.json";
import pt from "./locales/pt.json";
import hu from "./locales/hu.json";
import da from "./locales/da.json";
import sv from "./locales/sv.json";
import ro from "./locales/ro.json";
import tr from "./locales/tr.json";
import ru from "./locales/ru.json";
import uk from "./locales/uk.json";
import type { AccessTier, LogEntry, SupportedLocale } from "./types";

export type TranslationKey = keyof typeof en;
type Dictionary = Record<TranslationKey, string>;

const DICTIONARIES: Record<SupportedLocale, Dictionary> = {
  en,
  de,
  fr,
  es,
  pl,
  it,
  nl,
  cz,
  pt,
  hu,
  da,
  sv,
  ro,
  tr,
  ru,
  uk,
};

const LOCALE_FILLTYPE_INDEX: Record<SupportedLocale, number> = {
  de: 0,
  en: 1,
  fr: 2,
  es: 3,
  pl: 4,
  it: 5,
  nl: 6,
  cz: 7,
  pt: 8,
  hu: 9,
  da: 10,
  sv: 11,
  ro: 12,
  tr: 13,
  ru: 14,
  uk: 15,
};

/**
 * 16-language fallback lookup for official FS25 basegame fillTypes
 * (`[de, en, fr, es, pl, it, nl, cz, pt, hu, da, sv, ro, tr, ru, uk]`).
 * Sourced from GIANTS `data/maps/maps_fillTypes.xml` + `l10n_<lang>.xml` so historic
 * log lines, Great Demand badges, and Mock simulation items switch language dynamically live.
 */
const BASEGAME_FILLTYPE_NAMES: Record<string, readonly string[]> = {
  WHEAT: ["Weizen", "Wheat", "Blé", "Trigo", "Pszenica", "Frumento", "Tarwe", "Pšenice", "Trigo", "Búza", "Hvede", "Vete", "Grâu", "Buğday", "Пшеница", "Пшениця"],
  BARLEY: ["Gerste", "Barley", "Orge", "Cebada", "Jęczmień", "Orzo", "Gerst", "Ječmen", "Cevada", "Árpa", "Byg", "Korn", "Orz", "Arpa", "Ячмень", "Ячмінь"],
  OAT: ["Hafer", "Oat", "Avoine", "Avena", "Owies", "Avena", "Haver", "Oves", "Aveia", "Zab", "Havre", "Havre", "Ovăz", "Yulaf", "Овес", "Овес"],
  CANOLA: ["Raps", "Canola", "Colza", "Colza", "Rzepak", "Colza", "Koolzaad", "Řepka", "Canola", "Repce", "Raps", "Raps", "Rapiță", "Kanola", "Канола", "Ріпак"],
  SORGHUM: ["Hirse", "Sorghum", "Sorgho", "Sorgo", "Sorgo", "Sorgo", "Sorghum", "Čirok", "Sorgo", "Cirok", "Durra", "Durra", "Sorg", "Süpürge Darısı", "Сорго", "Сорго"],
  SUNFLOWER: ["Sonnenblumen", "Sunflowers", "Tournesols", "Girasoles", "Słoneczniki", "Girasoli", "Zonnebloemen", "Slunečnice", "Girassóis", "Napraforgó", "Solsikker", "Solrosor", "Floarea soarelui", "Ayçiçekleri", "Подсолнухи", "Соняшники"],
  SOYBEAN: ["Sojabohnen", "Soybeans", "Soja", "Soja", "Soja", "Semi di soia", "Sojabonen", "Sójové boby", "Soja", "Szójabab", "Sojabønner", "Sojabönor", "Boabe de soia", "Soya fasulyeleri", "Соевые бобы", "Соя"],
  MAIZE: ["Mais", "Corn", "Maïs", "Maíz", "Kukurydza", "Mais", "Maïs", "Kukuřice", "Milho", "Kukorica", "Majs", "Majs", "Porumb", "Mısır", "Кукуруза", "Кукурудза"],
  CORN: ["Mais", "Corn", "Maïs", "Maíz", "Kukurydza", "Mais", "Maïs", "Kukuřice", "Milho", "Kukorica", "Majs", "Majs", "Porumb", "Mısır", "Кукуруза", "Кукурудза"],
  POTATO: ["Kartoffeln", "Potatoes", "Pommes de terre", "Patatas", "Ziemniaki", "Patate", "Aardappels", "Brambory", "Batatas", "Burgonya", "Kartofler", "Potatis", "Cartofi", "Patates", "Картофель", "Картопля"],
  SUGARBEET: ["Zuckerrüben", "Sugar Beet", "Betteraves sucrières", "Remolacha azucarera", "Burak cukrowy", "Barbabietola da zucchero", "Suikerbieten", "Cukrová řepa", "Beterraba-sacarina", "Cukorrépa", "Sukkerroe", "Sockerbeta", "Sfeclă de zahăr", "Şeker Pancarı", "Свекла", "Цукровий буряк"],
  BEETROOT: ["Rote Beete", "Red Beet", "Betterave", "Remolacha", "Burak czerwony", "Barbabietola", "Rode biet", "Červená řepa", "Beterraba", "Cékla", "Rødbede", "Rödbeta", "Sfeclă", "Kırmızı Pancar", "Свёкла", "Буряк"],
  CARROT: ["Karotten", "Carrots", "Carottes", "Zanahorias", "Marchew", "Carote", "Wortels", "Mrkev", "Cenouras", "Sárgarépa", "Gulerødder", "Morötter", "Morcovi", "Havuç", "Морковь", "Морква"],
  PARSNIP: ["Pastinaken", "Parsnips", "Panais", "Chirivía", "Pasternak", "Pastinaca", "Pastinaak", "Pastinák", "Pastinaca", "Paszternák", "Pastinak", "Palsternacka", "Păstârnac", "Yabani Havuç", "Пастернак", "Пастернак"],
  COTTON: ["Baumwolle", "Cotton", "Coton", "Algodón", "Bawełna", "Cotone", "Katoen", "Bavlna", "Algodão", "Gyapot", "Bomuld", "Bomull", "Bumbac", "Pamuk", "Хлопок", "Бавовна"],
  RICELONGGRAIN: ["Langkornreis", "Long Grain Rice", "Riz long grain", "Arroz de grano largo", "Ryż długoziarnisty", "Riso a chicco lungo", "Langkorrelige rijst", "Dlouhozrnná rýže", "Arroz de Grão Longo", "Hosszú szemű rizs", "Langkornede ris", "Långkornigt ris", "Orez cu bob lung", "Uzun Taneli Pirinç", "Длиннозёрный рис", "Довгозернистий рис"],
  LONGGRAINRICE: ["Langkornreis", "Long Grain Rice", "Riz long grain", "Arroz de grano largo", "Ryż długoziarnisty", "Riso a chicco lungo", "Langkorrelige rijst", "Dlouhozrnná rýže", "Arroz de Grão Longo", "Hosszú szemű rizs", "Langkornede ris", "Långkornigt ris", "Orez cu bob lung", "Uzun Taneli Pirinç", "Длиннозёрный рис", "Довгозернистий рис"],
  RICE: ["Reis", "Rice", "Riz", "Arroz", "Ryż", "Riso", "Rijst", "Rýže", "Arroz", "Rizs", "Ris", "Ris", "Orez", "Pirinç", "Рис", "Рис"],
  GREENBEAN: ["Buschbohnen", "Green Beans", "Haricots verts", "Judías verdes", "Fasolka zielona", "Taccole", "Groene bonen", "Fazole", "Feijão-verde", "Zöldbab", "Grønne bønner", "Gröna bönor", "Fasole verde", "Taze Fasulye", "Стручковая фасоль", "Зелені боби"],
  PEA: ["Erbsen", "Peas", "Petits pois", "Guisantes", "Groszek", "Piselli", "Erwten", "Hrách", "Ervilhas", "Borsó", "Ærter", "Ärtor", "Mazăre", "Bezelye", "Горох", "Горох"],
  PEAS: ["Erbsen", "Peas", "Petits pois", "Guisantes", "Groszek", "Piselli", "Erwten", "Hrách", "Ervilhas", "Borsó", "Ærter", "Ärtor", "Mazăre", "Bezelye", "Горох", "Горох"],
  SPINACH: ["Spinat", "Spinach", "Épinards", "Espinacas", "Szpinak", "Spinaci", "Spinazie", "Špenát", "Espinafre", "Spenót", "Spinat", "Spenat", "Spanac", "Ispanak", "Шпинат", "Шпинат"],
  SUGARCANE: ["Zuckerrohr", "Sugarcane", "Canne à sucre", "Caña de azúcar", "Trzcina cukrowa", "Canna da zucchero", "Suikerriet", "Cukrová třtina", "Cana-de-açúcar", "Cukornád", "Sukkerrør", "Sockerrör", "Trestie de zahăr", "Şeker Kamışı", "Сахарный тростник", "Цукрова тростина"],
  WOOD: ["Holz", "Wood", "Bois", "Madera", "Drewno", "Legna", "Hout", "Dřevo", "Madeira", "Fa", "Træ", "Trä", "Lemn", "Kereste", "Дерево", "Деревина"],
  WOODCHIPS: ["Hackschnitzel", "Wood Chips", "Copeaux de bois", "Virutas", "Zrębki", "Cippato", "Houtsnippers", "Dřevní štěpka", "Lascas de Madeira", "Faforgács", "Træflis", "Träflis", "Așchii de lemn", "Talaş", "Щепа", "Тріска"],
  SUGARBEET_CUT: ["Zuckerrübenschnitzel", "Sugar Beet Cut", "Betteraves sucrières coupées", "Corte de remolacha azucarera", "Rozdrobnione buraki cukrowe", "Barbabietola da zucchero recisa", "Gesneden suikerbieten", "Odřezek cukrové řepy", "Corte de Beterraba-sacarina", "Cukorrépa – vágott", "Afskåret sukkerroe", "Skurna sockerbetor", "Bucată de sfeclă de zahăr", "Kesik Şeker Pancarı", "Резаная сахарная свёкла", "Різаний цукровий буряк"],
  SILAGE: ["Silage", "Silage", "Ensilage", "Ensilaje", "Kiszonka", "Insilato", "Kuilvoer", "Siláž", "Silagem", "Szilázs", "Ensilage", "Ensilage", "Nutreț însilozat", "Silaj", "Силос", "Силос"],
  GRASS: ["Gras", "Grass", "Herbe", "Hierba", "Trawa", "Erba", "Gras", "Tráva", "Erva", "Fű", "Græs", "Gräs", "Iarbă", "Çim", "Трава", "Трава"],
  MEADOW: ["Gras", "Grass", "Herbe", "Hierba", "Trawa", "Erba", "Gras", "Tráva", "Erva", "Fű", "Græs", "Gräs", "Iarbă", "Çim", "Трава", "Трава"],
  GRASS_WINDROW: ["Gras", "Grass", "Herbe", "Hierba", "Trawa", "Erba", "Gras", "Tráva", "Erva", "Fű", "Græs", "Gräs", "Iarbă", "Çim", "Трава", "Трава"],
  DRYGRASS: ["Gras", "Grass", "Herbe", "Hierba", "Trawa", "Erba", "Gras", "Tráva", "Erva", "Fű", "Græs", "Gräs", "Iarbă", "Çim", "Трава", "Трава"],
  DRYGRASS_WINDROW: ["Heu", "Hay", "Foin", "Heno", "Siano", "Fieno", "Hooi", "Seno", "Feno", "Széna", "Hø", "Hö", "Fân", "Kuru Ot", "Сено", "Сіно"],
  HAY: ["Heu", "Hay", "Foin", "Heno", "Siano", "Fieno", "Hooi", "Seno", "Feno", "Széna", "Hø", "Hö", "Fân", "Kuru Ot", "Сено", "Сіно"],
  STRAW: ["Stroh", "Straw", "Paille", "Paja", "Słoma", "Paglia", "Stro", "Sláma", "Palha", "Szalma", "Halm", "Halm", "Paie", "Saman", "Солома", "Солома"],
  WHEAT_CUT: ["Weizen", "Wheat", "Blé", "Trigo", "Pszenica", "Frumento", "Tarwe", "Pšenice", "Trigo", "Búza", "Hvede", "Vete", "Grâu", "Buğday", "Пшеница", "Пшениця"],
  BARLEY_CUT: ["Gerste", "Barley", "Orge", "Cebada", "Jęczmień", "Orzo", "Gerst", "Ječmen", "Cevada", "Árpa", "Byg", "Korn", "Orz", "Arpa", "Ячмень", "Ячмінь"],
  OAT_CUT: ["Hafer", "Oat", "Avoine", "Avena", "Owies", "Avena", "Haver", "Oves", "Aveia", "Zab", "Havre", "Havre", "Ovăz", "Yulaf", "Овес", "Овес"],
  CANOLA_CUT: ["Raps", "Canola", "Colza", "Colza", "Rzepak", "Colza", "Koolzaad", "Řepka", "Canola", "Repce", "Raps", "Raps", "Rapiță", "Kanola", "Канола", "Ріпак"],
  SOYBEAN_CUT: ["Sojabohnen", "Soybeans", "Soja", "Soja", "Soja", "Semi di soia", "Sojabonen", "Sójové boby", "Soja", "Szójabab", "Sojabønner", "Sojabönor", "Boabe de soia", "Soya fasulyeleri", "Соевые бобы", "Соя"],
  GRAPE: ["Trauben", "Grapes", "Raisin", "Uvas", "Winogrona", "Uva", "Druiven", "Hroznové víno", "Uvas", "Szőlő", "Vindruer", "Vindruvor", "Struguri", "Üzüm", "Виноград", "Виноград"],
  OLIVE: ["Oliven", "Olives", "Olives", "Aceitunas", "Oliwki", "Olive", "Olijven", "Olivy", "Azeitonas", "Olívabogyó", "Oliven", "Oliver", "Măsline", "Zeytin", "Оливки", "Оливки"],
  MILK: ["Milch", "Milk", "Lait", "Leche", "Mleko", "Latte", "Melk", "Mléko", "Leite", "Tej", "Mælk", "Mjölk", "Lapte", "Süt", "Молоко", "Молоко"],
  GOATMILK: ["Ziegenmilch", "Goat Milk", "Lait de chèvre", "Leche de cabra", "Kozie mleko", "Latte di capra", "Geitenmelk", "Kozí mléko", "Leite de cabra", "Kecsketej", "Gedemælk", "Getmjölk", "Lapte de capră", "Keçi Sütü", "Козье молоко", "Молоко кози"],
  BUFFALOMILK: ["Büffelmilch", "Buffalo Milk", "Lait de bufflonne", "Leche de búfala", "Bawole mleko", "Latte di bufala", "Buffelmelk", "Buvolí mléko", "Leite de búfala", "Vízibivalytej", "Bøffelmælk", "Buffelmjölk", "Lapte de bivoliță", "Manda Sütü", "Буйволиное молоко", "Молоко буйволиці"],
  WOOL: ["Wolle", "Wool", "Laine", "Lana", "Wełna", "Lana", "Wol", "Vlna", "Lã", "Gyapjú", "Uld", "Ull", "Lână", "Yün", "Шерсть", "Вовна"],
  EGG: ["Eier", "Eggs", "Œufs", "Huevos", "Jaja", "Uova", "Eieren", "Vejce", "Ovos", "Tojás", "Æg", "Ägg", "Ouă", "Yumurta", "Яйца", "Яйця"],
  LIQUIDSEEDTREATMENT: ["Beizmittel", "Seed Treating Liquid", "Liquide de traitement des semences", "Líquido para el tratamiento de semillas", "Płyn do zaprawiania nasion", "Liquido per il trattamento dei semi", "Zaadbehandelingsvloeistof", "Kapalina pro ošetření semen", "Líquido de Tratamento de Sementes", "Vetőmagkezelő folyadék", "Frøbehandlingsvæske", "Fröbehandlingsvätska", "Lichid pentru tratarea semințelor", "Tohum İşleme Sıvısı", "Жидкость для обработки семян", "Рідина для обробки насіння"],
  HONEY: ["Honig", "Honey", "Miel", "Miel", "Miód", "Miele", "Honing", "Med", "Mel", "Méz", "Honning", "Honung", "Miere", "Bal", "Мёд", "Мед"],
  LETTUCE: ["Salat", "Lettuce", "Laitue", "Lechuga", "Sałata", "Lattuga", "Sla", "Salát", "Alface", "Saláta", "Salat", "Sallad", "Salată", "Marul", "Салат", "Салат"],
  TOMATO: ["Tomaten", "Tomatoes", "Tomates", "Tomates", "Pomidory", "Pomodori", "Tomaten", "Rajčata", "Tomates", "Paradicsom", "Tomater", "Tomater", "Roșii", "Domatesler", "Помидоры", "Помідори"],
  STRAWBERRY: ["Erdbeeren", "Strawberries", "Fraises", "Fresas", "Truskawki", "Fragole", "Aardbeien", "Jahody", "Morangos", "Eper", "Jordbær", "Jordgubbar", "Căpșuni", "Çilekler", "Клубника", "Полуниця"],
  SPRING_ONION: ["Frühlingslauch", "Spring Onion", "Cébettes", "Cebolleta", "Zielona cebulka", "Cipollotto", "Lenteui", "Jarní cibulka", "Cebolinho", "Újhagyma", "Forårsløg", "Salladslök", "Ceapă verde", "Taze Soğan", "Зелёный лук", "Зелена цибуля"],
  NAPACABBAGE: ["Chinakohl", "Napa Cabbage", "Chou chinois", "Col china", "Kapusta pekińska", "Cavolo napa", "Napakool", "Zelí Napa", "Couve Napa", "Kínai kel", "Kinakål", "Salladskål", "Varză Napa", "Çin Lahanası", "Пекинская капуста", "Пекінська капуста"],
  CHILLI: ["Chilischoten", "Chili Peppers", "Piments", "Guindillas", "Papryczki chili", "Peperoncini", "Chilipepers", "Čili papričky", "Malaguetas", "Chilipaprikák", "Chilipebre", "Chilifrukter", "Ardei iuți", "Acı Biber", "Перцы чили", "Перець чілі"],
  GARLIC: ["Knoblauch", "Garlic", "Ail", "Ajo", "Czosnek", "Aglio", "Knoflook", "Česnek", "Alho", "Fokhagyma", "Hvidløg", "Vitlök", "Usturoi", "Sarımsak", "Чеснок", "Часник"],
  ENOKI: ["Enoki", "Enoki", "Enoki", "Enoki", "Enoki", "Funghi enoki", "Enoki", "Penízovka sametonohá", "Cogumelos enoki", "Enoki", "Enoki", "Enoki", "Ciupercă Enoki", "Enoki", "Эноки", "Енокі"],
  OYSTER: ["Austernpilz", "Oyster Mushroom", "Pleurote", "Champiñón ostra", "Boczniak ostrygowaty", "Fungo ostrica", "Oesterzwam", "Hlíva ústřičná", "Cogumelo-ostra", "Laskagomba", "Østershat", "Ostronmussling", "Ciupercă pleurotus", "İstiridye Mantarı", "Устричный гриб", "Глива"],
  BOARDS: ["Bretter", "Planks", "Planches", "Tablas", "Deski", "Assi", "Planken", "Prkna", "Tábuas", "Deszkák", "Planker", "Plankor", "Scânduri", "Kalaslar", "Доски", "Дошки"],
  PLANKS: ["Lange Bretter", "Planks Long", "Longues planches", "Tablas largas", "Długie deski", "Assi lunghe", "Lange planken", "Dlouhá prkna", "Tábuas Compridas", "Hosszú deszkák", "Lange planker", "Långa plankor", "Scânduri lungi", "Uzun Kalaslar", "Длинные доски", "Довгі дошки"],
  WOODBEAM: ["Holzbalken", "Wood Beam", "Poutre en bois", "Viga de madera", "Drewniana belka", "Trave di legno", "Houten balk", "Dřevěný trám", "Viga de Madeira", "Fagerenda", "Træbjælke", "Träbjälke", "Bârnă de lemn", "Tahta Kiriş", "Деревянная балка", "Брус"],
  PREFABWALL: ["Fertigwand", "Prefab Wall", "Mur préfabriqué", "Pared prefabricada", "Gotowa ściana", "Muro prefabbricato", "Prefabmuur", "Prefabrikovaná stěna", "Parede Pré-fabricada", "Előregyártott fal", "Vægelement", "Väggmodul", "Perete prefabricat", "Prefabrik Duvar", "Блочная стена", "Каркасна стіна"],
  FLOUR: ["Mehl", "Flour", "Farine", "Harina", "Mąka", "Farina", "Meel", "Mouka", "Farinha", "Liszt", "Mel", "Mjöl", "Făină", "Un", "Мука", "Борошно"],
  RICEFLOUR: ["Reismehl", "Rice Flour", "Farine de riz", "Harina de arroz", "Mąka ryżowa", "Farina di riso", "Rijstbloem", "Rýžová mouka", "Farinha de Arroz", "Rizsliszt", "Rismel", "Rismjöl", "Făină de orez", "Pirinç Unu", "Рисовая мука", "Рисове борошно"],
  BREAD: ["Brot", "Bread", "Pain", "Pan", "Chleb", "Pane", "Brood", "Chleba", "Pão", "Kenyér", "Brød", "Bröd", "Pâine", "Ekmek", "Хлеб", "Хліб"],
  CAKE: ["Kuchen", "Cake", "Gâteau", "Tarta", "Ciasto", "Torta", "Taart", "Dort", "Bolo", "Torta", "Kage", "Tårta", "Tort", "Kek", "Торт", "Торт"],
  MILK_BOTTLED: ["Milch (abgefüllt)", "Milk (bottled)", "Lait (en bouteille)", "Leche (embotellada)", "Mleko w butelkach", "Latte (in bottiglia)", "Melk (in fles)", "Mléko (v lahvích)", "Leite (engarrafado)", "Tej (üveges)", "Mælk (på flaske)", "Mjölk (på flaska)", "Lapte (îmbuteliat)", "Süt (şişelenmiş)", "Бутылки молока", "Молоко (у пляшках)"],
  GOATMILK_BOTTLED: ["Ziegenmilch (abgefüllt)", "Goat Milk (bottled)", "Lait de chèvre (en bouteille)", "Leche de cabra (embotellada)", "Kozie mleko w butelkach", "Latte di capra (in bottiglia)", "Geitenmelk (in fles)", "Kozí mléko (v lahvích)", "Leite de Cabra (engarrafado)", "Kecsketej (üveges)", "Gedemælk (på flaske)", "Getmjölk (på flaska)", "Lapte de capră (îmbuteliat)", "Keçi Sütü (şişelenmiş)", "Бутылки козьего молока", "Козяче молоко (у пляшках)"],
  BUFFALOMILK_BOTTLED: ["Büffelmilch (abgefüllt)", "Buffalo Milk (bottled)", "Lait de bufflonne (en bouteille)", "Leche de búfala (embotellada)", "Bawole mleko w butelkach", "Latte di bufala (in bottiglia)", "Buffelmelk (in fles)", "Buvolí mléko (v lahvích)", "Leite de Búfala (engarrafado)", "Vízibivalytej (üveges)", "Bøffelmælk (på flaske)", "Buffelmjölk (på flaska)", "Lapte de bivoliță (îmbuteliat)", "Manda Sütü (şişelenmiş)", "Бутылки буйволиного молока", "Буйволине молоко (у пляшках)"],
  BUTTER: ["Butter", "Butter", "Beurre", "Mantequilla", "Masło", "Burro", "Boter", "Máslo", "Manteiga", "Vaj", "Smør", "Smör", "Unt", "Tereyağı", "Масло", "Масло"],
  CHEESE: ["Käse", "Cheese", "Fromage", "Queso", "Ser żółty", "Formaggio", "Kaas", "Sýr", "Queijo", "Sajt", "Ost", "Ost", "Brânză", "Peynir", "Сыр", "Сир"],
  BUFFALOMOZZARELLA: ["Büffelmozzarella", "Buffalo Mozzarella", "Mozzarella de bufflonne", "Mozzarella de búfala", "Bawola mozzarella", "Mozzarella di bufala", "Buffelmozzarella", "Buvolí mozzarella", "Mozarela de Búfala", "Bivalymozzarella", "Bøffelmozzarella", "Buffelmozzarella", "Mozzarella de bivoliță", "Manda Mozerellası", "Буйволиная моцарелла", "Буйвіл Моцарелла"],
  GOATCHEESE: ["Ziegenkäse", "Goat cheese", "Fromage de chèvre", "Queso de cabra", "Kozi ser", "Formaggio di capra", "Geitenkaas", "Kozí sýr", "Queijo de cabra", "Kecskesajt", "Gedeost", "Getost", "Brânză de Capră", "Keçi peyniri", "Козий сыр", "Козячий сир"],
  CHOCOLATE: ["Schokolade", "Chocolate", "Chocolat", "Chocolate", "Czekolada", "Cioccolato", "Chocolade", "Čokoláda", "Chocolate", "Csokoládé", "Chokolade", "Choklad", "Ciocolată", "Çikolata", "Шоколад", "Шоколад"],
  POTATOCHIPS: ["Kartoffelchips", "Potato Chips", "Chips", "Patatas fritas", "Czipsy ziemniaczane", "Patatine", "Aardappelchips", "Bramborové lupínky", "Batatas Fritas", "Burgonyachips", "Kartoffelchips", "Potatischips", "Chipsuri de cartofi", "Patates Cipsi", "Картофельные чипсы", "Картопляні чіпси"],
  RICEROLLS: ["Reisrollen", "Rice Rolls", "Galettes de riz soufflé", "Tortitas de arroz", "Sajgonki", "Involtini di riso", "Rijstrolletjes", "Rýžové rolky", "Rolos de Arroz", "Rizstekercsek", "Rispapirsruller", "Risrullar", "Rulouri de orez", "Pirinç Patlağı", "Рисовые биточки", "Рисові хлібці"],
  FERMENTEDNAPACABBAGE: ["Kimchi", "Kimchi", "Kimchi", "Kimchi", "Kimchi", "Kimchi", "Kimchi", "Kimčchi", "Kimchi", "Kimcsi", "Kimchi", "Kimchi", "Kimchi", "Kimçi", "Кимчи", "Кімчі"],
  PRESERVEDCARROTS: ["Karottendosen", "Preserved Food Carrots", "Carottes en conserve", "Zanahoria en conserva", "Marchew konserwowa", "Carote in scatola", "Ingelegde wortels", "Konzervovaná mrkev", "Conserva de Cenoura", "Tartósított élelmiszer – sárgarépa", "Syltede gulerødder", "Konserverade morötter", "Morcovi conservați", "Konserve Havuç", "Консервированная морковь", "Консервована морква"],
  PRESERVEDPARSNIP: ["Pastinakendosen", "Preserved Food Parsnip", "Panais en conserve", "Chirivía en conserva", "Pasternak konserwowy", "Pastinaca in scatola", "Ingelegde pastinaken", "Konzervovaný pastinák", "Conserva de Pastinaca", "Tartósított élelmiszer – paszternák", "Syltede pastinakker", "Konserverade palsternackor", "Păstârnac conservat", "Konserve Yabani Havuç", "Консервированный пастернак", "Консервований пастернак"],
  PRESERVEDBEETROOT: ["Rote-Bete-Dosen", "Preserved Food Beetroot", "Betteraves en conserve", "Remolacha en conserva", "Buraki konserwowe", "Barbabietola in scatola", "Ingelegde rode bieten", "Konzervovaná červená řepa", "Conserva de Beterraba", "Tartósított élelmiszer – cékla", "Syltede rødbeder", "Konserverade rödbetor", "Sfeclă conservată", "Konserve Pancar", "Консервированная свёкла", "Консервований буряк"],
  NOODLESOUP: ["Nudelsuppe", "Noodle Soup", "Soupe de nouilles", "Sopa de fideos", "Zupa azjatycka", "Zuppa di noodle", "Noedelsoep", "Nudlová polévka", "Sopa de Massa", "Tésztaleves", "Nuddelsuppe", "Nudelsoppa", "Supă de tăieței", "Erişteli Çorba", "Суп-лапша", "Суп з локшиною"],
  SOUPCANSMIXED: ["Dreierlei-Suppe", "Triple Soup", "Soupe aux trois légumes", "Sopa triple", "Zupa z trzech składników", "Zuppa tris", "Soep met 3 groentes", "Trojitá polévka", "Sopa Tripla", "Triplaleves", "Trippelsuppe", "Trippelsoppa", "Supă triplă", "Üç Sebzeli Çorba", "Тройной суп", "Суп із трьох інгредієнтів"],
  SOUPCANSCARROTS: ["Karottensuppe", "Carrot Soup", "Soupe de carotte", "Sopa de zanahoria", "Zupa marchewkowa", "Zuppa di carote", "Wortelsoep", "Mrkvová polévka", "Sopa de Cenoura", "Sárgarépaleves", "Gulerodssuppe", "Morotssoppa", "Supă de morcovi", "Havuç Çorbası", "Морковный суп", "Морквяний суп"],
  SOUPCANSPARSNIP: ["Pastinakensuppe", "Parsnip Soup", "Soupe de panais", "Sopa de chirivía", "Zupa z pasternaku", "Zuppa di pastinaca", "Pastinaaksoep", "Pastináková polévka", "Sopa de Pastinaca", "Paszternákleves", "Pastinaksuppe", "Palsternackssoppa", "Supă de păstârnac", "Yabani Havuç Çorbası", "Пастернаковый суп", "Суп із пастернаку"],
  SOUPCANSBEETROOT: ["Rote-Beete-Suppe", "Red Beet Soup", "Soupe de betterave", "Sopa de remolacha", "Barszcz czerwony", "Zuppa di barbabietole", "Rode bietensoep", "Polévka z červené řepy", "Sopa de Beterraba", "Céklaleves", "Rødbedesuppe", "Rödbetssoppa", "Supă de sfeclă roșie", "Kırmızı Pancar Çorbası", "Красносвекольный суп", "Буряковий суп"],
  SOUPCANSPOTATO: ["Kartoffelsuppe", "Potato Soup", "Soupe de pomme de terre", "Sopa de patata", "Zupa ziemniaczana", "Zuppa di patate", "Aardappelsoep", "Bramborová polévka", "Sopa de Batata", "Burgonyaleves", "Kartoffelsuppe", "Potatissoppa", "Supă de cartofi", "Patates Çorbası", "Картофельный суп", "Картопляний суп"],
  FABRIC: ["Stoff", "Fabric", "Tissu", "Tejido", "Tkaniny", "Tessuto", "Stof", "Tkanina", "Tecido", "Anyag", "Stof", "Tyg", "Material", "Kumaş", "Ткань", "Тканина"],
  CLOTHES: ["Kleidung", "Clothes", "Vêtements", "Ropa", "Ubrania", "Abiti", "Kleding", "Oblečení", "Roupa", "Ruhadarabok", "Tøj", "Kläder", "Haine", "Kıyafetler", "Одежда", "Одяг"],
  SUGAR: ["Zucker", "Sugar", "Sucre", "Azúcar", "Cukier", "Zucchero", "Suiker", "Cukr", "Açúcar", "Cukor", "Sukker", "Socker", "Zahăr", "Şeker", "Сахар", "Цукор"],
  CEREAL: ["Müsli", "Cereal", "Céréales", "Cereales", "Płatki zbożowe", "Cereali", "Ontbijtgranen", "Obiloviny", "Cereal", "Müzli", "Morgenmadsprodukt", "Flingor", "Cereale", "Mısır Gevreği", "Кукурузные хлопья", "Пластівці"],
  FURNITURE: ["Möbel", "Furniture", "Mobilier", "Muebles", "Meble", "Mobili", "Meubels", "Nábytek", "Mobília", "Bútor", "Møbler", "Möbler", "Mobilă", "Mobilya", "Мебель", "Меблі"],
  CARTONROLL: ["Kartonrolle", "Carton Roll", "Rouleau de carton", "Rollo de cartón", "Rolka tektury", "Rotolo di cartone", "Rol karton", "Role lepenky", "Rolo de Cartão", "Kartontekercs", "Rulle med pap", "Papprulle", "Rolă de carton", "Karton Tomarı", "Рулон картона", "Рулон картону"],
  PAPERROLL: ["Papierrolle", "Paper Roll", "Rouleau de papier", "Rollo de papel", "Rolka papieru", "Rotolo di carta", "Rol papier", "Role papíru", "Rolo de Papel", "Papírtekercs", "Papirrulle", "Pappersrulle", "Rolă de hârtie", "Kağıt Tomarı", "Рулон бумаги", "Рулон паперу"],
  PIANO: ["Piano", "Piano", "Piano", "Piano", "Fortepian", "Pianoforte", "Piano", "Piano", "Piano", "Zongora", "Klaver", "Piano", "Pian", "Piyano", "Фортепьяно", "Піаніно"],
  WAGON: ["Anhänger", "Wagon", "Wagon", "Remolque", "Wóz", "Cassone", "Huifkar", "Vůz", "Atrelado", "Vagon", "Vogn", "Vagn", "Remorcă", "Vagon", "Повозка", "Візок"],
  TOYTRACTOR: ["Spielzeugtraktor", "Toy Tractor", "Tracteur jouet", "Tractor de juguete", "Zabawkowy traktor", "Trattore giocattolo", "Speelgoedtractor", "Traktor na hraní", "Trator de brinquedo", "Játéktraktor", "Legetøjstraktor", "Leksakstraktor", "Tractor de jucărie", "Oyuncak Traktör", "Игрушечный трактор", "Іграшковий трактор"],
  SUNFLOWER_OIL: ["Sonnenblumenöl", "Sunflower Oil", "Huile de tournesol", "Aceite de girasol", "Olej słonecznikowy", "Olio di girasole", "Zonnebloemolie", "Slunečnicový olej", "Óleo de Girassol", "Napraforgóolaj", "Solsikkeolie", "Solrosolja", "Ulei de floarea-soarelui", "Ayçiçeği Yağı", "Подсолнечное масло", "Соняшникова олія"],
  CANOLA_OIL: ["Rapsöl", "Canola Oil", "Huile de colza", "Aceite de colza", "Olej rzepakowy", "Olio di colza", "Koolzaadolie", "Řepkový olej", "Óleo de colza", "Repceolaj", "Rapsolie", "Rapsolja", "Ulei de rapiță", "Kanola Yağı", "Рапсовое масло", "Ріпакова олія"],
  OLIVE_OIL: ["Olivenöl", "Olive Oil", "Huile d'olive", "Aceite de oliva", "Oliwa z oliwek", "Olio d'oliva", "Olijfolie", "Olivový olej", "Azeite", "Olívaolaj", "Olivenolie", "Olivolja", "Ulei de măsline", "Zeytinyağı", "Оливковое масло", "Оливкова олія"],
  RICE_OIL: ["Reisöl", "Rice Oil", "Huile de riz", "Aceite de arroz", "Olej ryżowy", "Olio di riso", "Rijstolie", "Rýžový olej", "Óleo de arroz", "Rizsolaj", "Risolie", "Risolja", "Ulei de orez", "Pirinç Yağı", "Рисовое масло", "Рисова олія"],
  RAISINS: ["Rosinen", "Raisins", "Raisins secs", "Pasas", "Rodzynki", "Uvetta", "Krenten", "Hrozinky", "Passas", "Mazsolák", "Rosiner", "Russin", "Stafide", "Kuru Üzüm", "Изюм", "Родзинки"],
  GRAPEJUICE: ["Traubensaft", "Grape Juice", "Jus de raisin", "Zumo de uva", "Sok z winogron", "Succo d'uva", "Druivensap", "Hroznový džus", "Sumo de Uva", "Szőlőlé", "Druesaft", "Druvjuice", "Suc de struguri", "Üzüm Suyu", "Виноградный сок", "Виноградний сік"],
  CEMENT: ["Zementsäcke", "Cement bags", "Sacs de ciment", "Sacos de cemento", "Worki z cementem", "Sacchi di cemento", "Cementzakken", "Pytle s cementem", "Sacos de cimento", "Cementzsákok", "Cementsække", "Cementsäckar", "Saci de ciment", "Çimento torbaları", "Мешки с цементом", "Мішки із цементом"],
  BARREL: ["Fass", "Barrel", "Tonneau", "Barril", "Beczka", "Cisterna", "Vat", "Sud", "Barril", "Hordó", "Tønde", "Tunna", "Butoi", "Fıçı", "Бочка", "Бочка"],
  BATHTUB: ["Badewanne", "Bathtub", "Baignoire", "Bañera", "Wanna", "Vasca da bagno", "Badkuip", "Vana", "Banheira", "Fürdőkád", "Badekar", "Badkar", "Cadă", "Küvet", "Ванна", "Ванна"],
  BUCKET: ["Eimer", "Bucket", "Seau", "Cubo", "Wiadro", "Secchio", "Emmer", "Vědro", "Balde", "Vödör", "Spand", "Skopa", "Găleată", "Kepçe", "Ведро", "Відро"],
  ROOFPLATES: ["Dachplatte", "Roof Plate", "Tôle de toiture", "Chapa de tejado", "Pokrycie dachowe", "Piastra per tetto", "Dakplaat", "Střešní krytina", "Placa de Telhado", "Tetőlemez", "Tagplade", "Takplatta", "Placă de acoperiș", "Çatı Plakası", "Кровельная панель", "Покрівельна плита"],
  ROPE: ["Seil", "Rope", "Corde", "Cuerda", "Sznur", "Fune", "Touw", "Lano", "Corda", "Kötél", "Reb", "Rep", "Sfoară", "Halat", "Верёвка", "Трос"],
  CANNED_PEAS: ["Dosenerbsen", "Canned Peas", "Petit pois en conserve", "Guisantes en conserva", "Puszkowany groszek", "Piselli in scatola", "Erwten in blik", "Hrášek v konzervě", "Ervilhas Enlatadas", "Borsókonzerv", "Dåseærter", "Konserverade ärtor", "Conservă de mazăre", "Konserve Bezelye", "Консервированный горошек", "Горошок консервований"],
  CEMENTBRICKS: ["Betonziegel", "Cement Brick", "Briques en ciment", "Ladrillo de cemento", "Cementowa cegła", "Mattone di cemento", "Cementsteen", "Betonová cihla", "Tijolo de Cimento", "Cementtégla", "Cementmursten", "Cementblock", "Bolțar", "Çimento Tuğla", "Цементный кирпич", "Цегла із цементу"],
  SPINACH_BAGS: ["Spinatsack", "Spinach Bag", "Sac d'épinards", "Saco de espinacas", "Worek szpinaku", "Sacco di spinaci", "Spinaziezak", "Pytlík špenátu", "Saco de Espinafres", "Spenótos zsák", "Spinatpose", "Säck med spenat", "Pungă de spanac", "Ispanak Çuvalı", "Пакет шпината", "Мішок шпинату"],
  JARRED_GREENBEAN: ["Eingemachte Buschbohnen", "Jarred Green Bean", "Bocal de haricots verts", "Judías verdes en frasco", "Słoikowana fasolka zielona", "Taccole in barattolo", "Groene bonen in pot", "Fazole ve sklenici", "Feijão-verde em Frasco", "Savanyított zöldbab", "Grønne bønner i glas", "Gröna bönor på burk", "Borcan cu fasole verde", "Kavanozda Taze Fasulye", "Стручковая фасоль в банках", "Стручкова квасоля в банці"],
  RICE_BAGS: ["Reissack", "Rice Bag", "Sac de riz", "Saco de arroz", "Worek ryżu", "Sacco di riso", "Rijstzak", "Pytlík rýže", "Saco de Arroz", "Rizses zsák", "Rispose", "Säck med ris", "Pungă de orez", "Pirinç Çuvalı", "Мешок риса", "Мішок рису"],
  RICE_BOXES: ["Reisbox", "Rice Box", "Boîte de riz", "Caja de arroz", "Pudełko ryżu", "Confezione di riso", "Rijstdoos", "Krabice rýže", "Caixa de Arroz", "Rizses doboz", "Riskasse", "Rislåda", "Cutie de orez", "Pirinç Kutusu", "Коробка риса", "Ящик рису"],
  SEEDS: ["Saatgut", "Seeds", "Semences", "Semillas", "Nasiona", "Semi", "Zaden", "Osiva", "Sementes", "Magok", "Frø", "Frön", "Semințe", "Tohumlar", "Семена", "Насіння"],
  STONE: ["Steine", "Stones", "Pierres", "Piedras", "Kamienie", "Sassi", "Stenen", "Kameny", "Pedras", "Kő", "Sten", "Stenar", "Pietre", "Taşlar", "Камни", "Каміння"],
  MANURE: ["Mist", "Manure", "Fumier", "Estiércol", "Obornik", "Letame", "Mest", "Mrva", "Estrume", "Trágya", "Møg", "Naturgödsel", "Bălegar", "Doğal Gübre", "Навоз", "Гній"],
  LIQUIDMANURE: ["Gülle", "Slurry", "Lisier", "Purín", "Gnojówka", "Liquami", "Drijfmest", "Močůvka", "Chorume", "Hígtrágya", "Gylle", "Flytgödsel", "Îngrășământ semilichid", "Bulamaç", "Жидкий навоз", "Гнойова жижа"],
  DIGESTATE: ["Gärreste", "Digestate", "Digestat", "Digestato", "Poferment", "Digestato", "Digestaat", "Digestát", "Biofertilizante", "Fermentált", "Fermentat", "Rötrester", "Digestat", "Organik gübre", "Дигестат", "Дигестат"],
  FORAGE: ["Totalmischration", "Total Mixed Ration", "Ration totale mélangée", "Ración mezclada", "Pasza TMR", "Razione mista totale", "Compleet gemengd rantsoen", "Směsná krmná dávka", "Ração Mista Total", "Teljes takarmánykeverék", "TMR", "Fullfoder", "Rație mixtă totală", "Tam Karışık Tayın", "Моносмесь", "Повний змішаний раціон"],
  FORAGE_MIXING: ["Futter", "Forage", "Fourrage", "Forraje", "Pasza", "Foraggio", "Veevoer", "Píce", "Forragem", "Takarmány", "Foder", "Grovfoder", "Furaj", "Yemlik", "Фураж", "Корми"],
  WATER: ["Wasser", "Water", "Eau", "Agua", "Woda", "Acqua", "Water", "Voda", "Água", "Víz", "Vand", "Vatten", "Apă", "Su", "Вода", "Вода"],
  CHAFF: ["Häckselgut", "Chaff", "Paille hachée", "Paja fina", "Sieczka", "Pula", "Kaf", "Řezanka", "Restolho", "Szecska", "Hakkelse", "Agnar", "Pleavă", "Anız", "Сечка", "Полова"],
  TREESAPLINGS: ["Setzlinge", "Tree Saplings", "Plants d'arbres", "Plantones", "Sadzonki", "Piantine", "Boomzaailingen", "Sazenice stromů", "Mudas de Árvore", "Facsemeték", "Unge træer", "Telningar", "Puieți", "Ağaç Fidanları", "Саженцы", "Саджанці"],
  TREE: ["Baum", "Tree", "Arbre", "Árbol", "Drzewo", "Albero", "Boom", "Strom", "Árvore", "Fa", "Træ", "Träd", "Copac", "Ağaç", "Дерево", "Дерево"],
  OILSEEDRADISH: ["Ölrettich", "Oilseed Radish", "Radis oléagineux", "Rábano oleífero", "Rzodkiew oleista", "Rafano oleifero", "Bladrammenas", "Ředkev olejná", "Rabanete Japonês", "Olajretek", "Olieræddike", "Rättika", "Ridichi", "Yağ Tohumu Turpu", "Посевная редька", "Олійна редька"],
  POPLAR: ["Pappel", "Poplar", "Peuplier", "Álamo", "Topola", "Pioppo", "Populier", "Topol", "Choupo", "Nyárfa", "Poppel", "Poppel", "Plop", "Kavak", "Тополь", "Тополя"],
  DIESEL: ["Diesel", "Diesel", "Diesel", "Diésel", "Olej napędowy", "Diesel", "Diesel", "Nafta", "Diesel", "Dízel", "Diesel", "Diesel", "Motorină", "Dizel", "Дизельное топливо", "Дизель"],
  DEF: ["Synthetische Harnstofflösung", "Diesel exhaust fluid", "Fluide d'échappement diesel", "Fluido de escape de diésel", "Wodny roztwór mocznika", "Additivo per Diesel", "Dieseluitlaatvloeistof", "Kapalina pro čištění výfukových plynů naftových motorů", "Fluido de escape diesel", "Dízel kipufogófolyadék", "Dieseludstødningsvæske", "Dieselavgasvätska", "Aditiv diesel", "Dizel egzoz sıvısı", "Реагент для очистки выхлопов", "Рідина для очищення дизельних вихлопних газів"],
  AIR: ["Luft", "Air", "Air", "Aire", "Powietrze", "Aria", "Lucht", "Vzduch", "Ar", "Levegő", "Luft", "Luft", "Aer", "Hava", "Воздух", "Повітря"],
  ELECTRICCHARGE: ["Elektrische Ladung", "Electric Charge", "Recharge électrique", "Carga eléctrica", "Ładowanie elektryczne", "Carica elettrica", "Elektrische lading", "Elektrické nabití", "Carga Elétrica", "Elektromos töltés", "Elektrisk opladning", "Elektrisk laddning", "Încărcare electrică", "Elektrik Yükü", "Электрозаряд", "Електричний заряд"],
  METHANE: ["Methan", "Methane", "Méthane", "Metano", "Metan", "Metano", "Methaan", "Metan", "Metano", "Metán", "Metan", "Metan", "Metan", "Metan", "Метан", "Метан"],
  SNOW: ["Schnee", "Snow", "Neige", "Nieve", "Śnieg", "Neve", "Sneeuw", "Sníh", "Neve", "Hó", "Sne", "Snö", "Zăpadă", "Kar", "Снег", "Сніг"],
  ROADSALT: ["Streusalz", "Road Salt", "Sel de déneigement", "Sal para carreteras", "Sól drogowa", "Sale antigelo", "Strooizout", "Posypová sůl", "Sal de Estrada", "Útszóró só", "Vejsalt", "Vägsalt", "Sare pentru deszăpezire", "Yol Tuzu", "Дорожная соль", "Технічна сіль"],
  ROUNDBALE: ["Rundballen", "Round bale", "Balle ronde", "Paca redonda", "Bela", "Rotoballa", "Ronde baal", "Válcový balík", "Fardo Redondo", "Körbála", "Rund balle", "Rundbal", "Balot rotund", "Rulo balya", "Круглый тюк", "Круглий тюк"],
  ROUNDBALE_GRASS: ["Rundballen (Gras)", "Round bale grass", "Balle d'herbe ronde", "Hierba para pacas redondas", "Bela trawy", "Rotoballa di erba", "Ronde grasbaal", "Válcový balík trávy", "Fardo Redondo de Erva", "Körbála – fű", "Rund græsballe", "Rund gräsbal", "Balot rotund de iarbă", "Rulo balya çim", "Круглый тюк травы", "Круглий тюк трави"],
  ROUNDBALE_DRYGRASS: ["Rundballen (Heu)", "Round bale hay", "Balle de foin ronde", "Heno para pacas redondas", "Bela siana", "Rotoballa di fieno", "Ronde hooibaal", "Válcový balík sena", "Fardo Redondo de Feno", "Körbála – széna", "Rund høballe", "Rund höbal", "Balot rotund de fân", "Kuru ot rulo balyası", "Круглый тюк сена", "Круглий тюк сіна"],
  ROUNDBALE_COTTON: ["Rundballen (Baumwolle)", "Round bale cotton", "Balle de coton ronde", "Algodón para pacas redondas", "Bela bawełny", "Rotoballa di cotone", "Ronde katoenbaal", "Válcový balík bavlny", "Fardo Redondo de Algodão", "Körbála – gyapot", "Rund bomuldsballe", "Rund bomullsbal", "Balot rotund de bumbac", "Rulo balya pamuk", "Круглый тюк хлопка", "Круглий тюк бавовни"],
  ROUNDBALE_WOOD: ["Rundballen (Holz)", "Round bale wood", "Balle de bois ronde", "Madera para pacas redondas", "Bela drewna", "Rotoballa di legno", "Ronde houtbaal", "Válcový balík dřeva", "Fardo Redondo de Madeira", "Körbála – fa", "Rund træballe", "Rund träbal", "Balot rotund de lemn", "Rulo balya tahta", "Круглый тюк дерева", "Круглий тюк деревини"],
  SQUAREBALE: ["Quaderballen", "Square bale", "Balle carrée", "Paca rectangular", "Kostka", "Balla quadrata", "Vierkante baal", "Hranatý balík", "Fardo quadrado", "Kockabála", "Firkantet balle", "Fyrkantig bal", "Balot pătrat", "Kare balya", "Квадратный тюк", "Квадратний тюк"],
  SQUAREBALE_GRASS: ["Quaderballen (Gras)", "Square bale grass", "Balle d'herbe carrée", "Hierba para pacas rectangulares", "Kostka trawy", "Balla quadrata di erba", "Vierkante grasbaal", "Hranatý balík trávy", "Fardo quadrado de erva", "Kockabála – fű", "Firkantet græsballe", "Fyrkantig gräsbal", "Balot pătrat de iarbă", "Kare balya çim", "Квадратный тюк травы", "Квадратний тюк трави"],
  SQUAREBALE_DRYGRASS: ["Quaderballen (Heu)", "Square bale hay", "Balle de foin carrée", "Heno para pacas rectangulares", "Kostka siana", "Balla quadrata di fieno", "Vierkante hooibaal", "Hranatý balík sena", "Fardo quadrado de feno", "Kockabála – széna", "Firkantet høballe", "Fyrkantig höbal", "Balot pătrat de fân", "Kuru ot kare balyası", "Квадратный тюк сена", "Квадратний тюк сіна"],
  SQUAREBALE_COTTON: ["Quaderballen (Baumwolle)", "Square bale cotton", "Balle de coton carrée", "Algodón para pacas rectangulares", "Kostka bawełny", "Balla quadrata di cotone", "Vierkante katoenbaal", "Hranatý balík bavlny", "Fardo quadrado de algodão", "Kockabála – gyapot", "Firkantet bomuldsballe", "Fyrkantig bomullsbal", "Balot pătrat de bumbac", "Kare balya pamuk", "Квадратный тюк хлопка", "Квадратний тюк бавовни"],
  SQUAREBALE_WOOD: ["Quaderballen (Holz)", "Square bale wood", "Balle de bois carrée", "Madera para pacas rectangulares", "Kostka drewna", "Balla quadrata di legno", "Vierkante houtbaal", "Hranatý balík dřeva", "Fardo quadrado de madeira", "Kockabála – fa", "Firkantet træballe", "Fyrkantig träbal", "Balot pătrat de lemn", "Kare balya tahta", "Квадратный тюк дерева", "Квадратний тюк деревини"],
  BALE_WRAP: ["Wickelfolie", "Bale Wrap", "Film à balles", "Encintado de pacas", "Folia do owijania", "Fasciatura per balle", "Balenwikkel", "Obal na balíky", "Película para fardos", "Bálacsomagolás", "Balleemballering", "Balinplastare", "Ambalaj balot", "Balya Paketi", "Тюковая обёртка", "Обмотування для тюків"],
  BALE_NET: ["Rundballennetz", "Round bale Net", "Filet à balles rondes", "Malla para pacas redondas", "Siatka do okragłych bel", "Rete per rotoballe", "Ronde balen-net", "Síť na válcové balíky", "Rede de fardo redondo", "Körbálaháló", "Rundballenet", "Nät för rundbalar", "Plasă balot rotund", "Rulo Balya Ağı", "Сеть для рулонных тюков", "Сітка для круглих тюків"],
  BALE_TWINE: ["Bindegarn für Quaderballen", "Square bale Twine", "Ficelle à balles carrées", "Cordel para pacas rectangulares", "Sznurek do kostek", "Spago per balle quadrate", "Vierkante balen-draad", "Motouz na hranaté balíky", "Barbante de fardo quadrado", "Kockabálazsineg", "Tråd til firkantede baller", "Tråd för fyrkantsbalar", "Sfoară balot pătrat", "Kare Balya İpi", "Бечёвка для кубических тюков", "Мотузка для квадратних тюків"],
  RICESAPLINGS: ["Reis Setzlinge", "Rice Saplings", "Plants de riz", "Plantones de arroz", "Sadzonki ryżu", "Piantine di riso", "Rijstzaailingen", "Sazenice rýže", "Mudas de arroz", "Rizscsemete", "Unge risplanter", "Ristelningar", "Puieți de orez", "Pirinç Fideleri", "Рисовые саженцы", "Саджанці рису"],
  FERTILIZER: ["Mineraldünger", "Solid Fertilizer", "Engrais solide", "Abonadora", "Nawóz stały", "Fertilizzante solido", "Vaste meststof", "Pevné hnojivo", "Fertilizante Sólido", "Szilárd műtrágya", "Fast gødning", "Fast gödsel", "Îngrășământ solid", "Katı gübre", "Удобрение", "Тверде добриво"],
  LIQUIDFERTILIZER: ["Flüssigdünger", "Liquid Fertilizer", "Engrais liquide", "Abono líquido", "Płynny nawóz", "Fertilizzante liquido", "Vloeibare meststof", "Tekuté hnojivo", "Fertilizante Líquido", "Folyékony műtrágya", "Flydende gødning", "Flytande gödsel", "Îngrășământ lichid", "Sıvı Gübre", "Жидкое удобрение", "Рідке добриво"],
  PIGFOOD: ["Schweinefutter", "Pig Food", "Nourriture pour cochons", "Comida para cerdos", "Karma dla świń", "Cibo per suini", "Varkensvoer", "Prasečí krmivo", "Ração para Porcos", "Sertéstakarmány", "Grisefoder", "Grismat", "Hrană pentru porci", "Domuz Besini", "Корм для свиней", "Корм для свиней"],
  TARP: ["Abdeckplane", "Tarp", "Bâche", "Lona", "Plandeka", "Tela", "Afdekzeil", "Plachta", "Cobertura", "Ponyva", "Presenning", "Presenning", "Prelată", "Branda", "Брезент", "Тент"],
  LIME: ["Kalk", "Lime", "Chaux", "Cal", "Wapno", "Calce viva", "Landbouwkalk", "Vápno", "Cal", "Mész", "Kalk", "Lime", "Var", "Kireç", "Известь", "Вапно"],
  HERBICIDE: ["Herbizid", "Herbicide", "Herbicide", "Herbicida", "Herbicyd", "Erbicida", "Onkruidverdelger", "Herbicid", "Herbicida", "Gyomirtó", "Ukrudtsmiddel", "Ogräsmedel", "Erbicid", "Herbisit", "Гербицид", "Гербіцид"],
  SILAGE_ADDITIVE: ["Siliermittel", "Silage Additive", "Additifs d'ensilage", "Aditivo de ensilaje", "Dodatek do kiszonek", "Additivi per insilato", "Kuilvoertoevoeging", "Silážní aditivum", "Aditivo para Silagem", "Szilázsadalék", "Ensilagetilsætningsstoffer", "Ensilagetillsats", "Aditivi pentru nutreț însilozat", "Silaj Takviyesi", "Силосная добавка", "Силосна добавка"],
  MINERAL_FEED: ["Mineralfutter", "Mineral Feed", "Complément minéral", "Pienso mineral", "Pasza mineralna", "Mangime minerale", "Mineraalvoer", "Minerální příměs", "Alimento Mineral", "Ásványi takarmány", "Mineralfoder", "Mineralfoder", "Hrană minerală", "Mineralli Yem", "Минеральный корм", "Мінеральний корм"],
  WEED: ["Unkraut", "Weed", "Mauvaises herbes", "Hierba", "Chwasty", "Erbaccia", "Onkruid", "Plevel", "Erva Daninha", "Gyom", "Ukrudt", "Ogräs", "Buruieni", "Yabani Ot", "Сорняк", "Бур’ян"],
  HORSE_PALOMINO: ["Isabell", "Palomino", "Cheval (palomino)", "Palomino", "Koń (izabelowaty)", "Palomino", "Paard (beige)", "Palomino", "Bege", "Bézs", "Palomino", "Palomino", "Bej", "İzabel", "Соловая", "Соловий"],
  HORSE_BLACK: ["Rappe", "Black", "Cheval (noir)", "Negro", "Koń (kary)", "Nero", "Zwart", "Vraník", "Preto", "Fekete", "Sort", "Svart", "Negru", "Yağız", "Вороная", "Вороний"],
  HORSE_BAY: ["Brauner", "Bay", "Cheval (bai)", "Castaño", "Koń (gniady)", "Baio", "Paard (bruin)", "Hnědák", "Castanho", "Barna", "Brun", "Brun", "Brun", "Doru", "Кауровая", "Гнідий"],
  HORSE_PINTO: ["Schecke", "Pinto", "Cheval (pie)", "Pío", "Koń (gniadosrokaty)", "Pezzato", "Paard (bruin-wit)", "Pinto", "Castanho e Branco", "Barna-fehér", "Pinto", "Skäck", "Brun cu alb", "Alaca", "Пегая", "Рябий"],
  HORSE_SEAL_BROWN: ["Schwarzbrauner", "Seal brown", "Cheval (bai-brun)", "Pardo", "Koń (ciemnogniady)", "Marrone scuro", "Paard (donkerbruin)", "Černý hnědák", "Castanho Escuro", "Sötétbarna", "Mørkebrun", "Mörkbrun", "Brun închis", "Kestane", "Караковая", "Караковий"],
  HORSE_GRAY: ["Schimmel", "Gray", "Cheval (gris)", "Tordo", "Koń (siwy)", "Grigio", "Paard (grijs)", "Šedák", "Cinzento", "Szürke", "Grå", "Skimmel", "Gri", "Kır", "Серая", "Сірий"],
  HORSE_DUN: ["Falbe", "Dun", "Cheval (isabelle)", "Dun", "Koń (bułany)", "Marrone chiaro", "Paard (lichtbruin)", "Plavák", "Castanho Claro", "Világosbarna", "Dunfarvet", "Black", "Brun deschis", "Boz", "Саврасая", "Саврасий"],
  HORSE_CHESTNUT: ["Fuchs", "Chestnut", "Cheval (alezan)", "Alazán", "Koń (kasztanowaty)", "Rosso marrone", "Paard (roodbruin)", "Ryzák", "Castanho Avermelhado", "Vörösbarna", "Fuks", "Fux", "Roșcat", "Al", "Рыжая", "Рудий"],
  COW_SWISS_BROWN: ["Brown-Swiss", "Brown-Swiss", "Brun noir", "Parda suiza", "Brown Swiss", "Bruna svizzera", "Brown Swiss", "Švýcarská hnědá", "Brown-Swiss", "Barna svájci", "Schweizisk brun", "Brown Swiss", "Brună elvețiană", "Montofon", "Бурая швицкая", "Швіцька порода"],
  COW_HOLSTEIN: ["Holstein", "Holstein", "Holstein", "Holstein", "Holsztyno-fryzyjska", "Holstein", "Holstein", "Holštýnská", "Holstein", "Holstein", "Holstein", "Holstein", "Holstein", "Holstein", "Голштинская", "Голштинська порода"],
  COW_LIMOUSIN: ["Limousin", "Limousin", "Limousine", "Limousin", "Limousine", "Limousine", "Limousin", "Limousinská", "Limousin", "Limousin", "Limousine", "Limousin", "Limousin", "Limuzin", "Лимузин", "Лімузинська порода"],
  COW_ANGUS: ["Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Angus", "Ангус", "Абердино-ангуська худоба"],
  COW_WATERBUFFALO: ["Wasserbüffel", "Water Buffalo", "Buffle", "Búfalo de agua", "Wół domowy", "Bufalo d'acqua", "Waterbuffel", "Buvol indický", "Búfalo-asiático", "Vízibivaly", "Vandbøffel", "Vattenbuffel", "Bivol de apă", "Manda", "Буйвол", "Водяний буйвіл"],
  COW_HIGHLAND_CATTLE: ["Hochlandrind", "Highland Cattle", "Bétail des Highlands", "Ganado de las tierras altas", "Szkocka rasa wyżynna", "Bestiame delle Highland", "Hooglandvee", "Skotský náhorní skot", "Gado das terras altas", "Felföldi szarvasmarha", "Højlandskvæg", "Skotsk höglandsboskap", "Vaci de munte", "İskoçya Sığırı", "Высокогорный скот", "Горянська худоба"],
  COW_BRAHMAN: ["Brahman", "Brahman", "Brahmane", "Brahman", "Brahman", "Bramano", "Brahman", "Brahman", "Brahman", "Brahma marha", "Brahman", "Brahman", "Brahman", "Brahman Sığırı", "Брахман", "Брахман"],
  SHEEP_LANDRACE: ["Bentheimer Landschaf", "Landrace of Bentheim", "Bentheim", "Landrace de Bentheim", "Landrace z Bentheim", "Landrace di Bentheim", "Landras van Bentheim", "Bentheimer Landschaf", "Landrace de Bentheim", "Bentheimer landrace", "Bentheims landrace", "Bentheim", "Landrace de Bentheim", "Bentheim Yerli", "Бентгеймская ландрас", "Бентхайм ландрас"],
  SHEEP_SWISS_MOUNTAIN: ["Schwarzbraunes Bergschaf", "Swiss Black-Brown Mountain", "Brun noir du Jura", "Marrón de las montañas de Gales", "Szwajcarska czarno-brązowa górska", "Nero bruna svizzera di montagna", "Schwarzbraunes Bergschaf", "Švýcarská černohnědá horská", "Schwarzbraunes Bergschaf", "Svájci fekete-barna hegyi", "Schwarzbraunes Bergschaf", "Swiss Black-Brown Mountain", "Elvețiană neagră-brună de munte", "İsviçre Siyah - Kahverengi Dağ", "Швейцарская чернобурая горная", "Чорно-коричневий бергсхаф"],
  SHEEP_STEINSCHAF: ["Alpines Steinschaf", "Steinschaf", "Mouton des Alpes", "Steinschaf", "Steinschaf", "Steinschaf", "Steinschaf", "Steinschaf", "Steinschaf", "Steinschaf", "Steinschaf", "Steinschaf", "Steinschaf", "Steinschaf", "Штайншаф", "Штайншаф"],
  SHEEP_BLACK_WELSH: ["Black Welsh Mountain", "Black Welsh Mountain", "Mouton de montagne noir gallois", "Negra de las montañas de Gales", "Walijska górska", "Di montagna del Galles", "Black Welsh Mountain", "Černá velšská horská", "Ovelha da montanha galesa preta", "Fekete walesi hegyi", "Black Welsh Mountain", "Black Welsh Mountain", "Galeză neagră de munte", "Siyah Galli Dağ", "Уэльская чёрная горная", "Чорна валлійська гірська"],
  PIG_LANDRACE: ["Deutsches Edelschwein", "German Landrace", "Landrace allemand", "German Landrace", "Niemiecka rasa krajowa", "Landrace tedesco", "Duits landras", "Německé landrase", "Landrace Alemão", "Német landrace", "Tysk landrace", "Tysk lantras", "Landrace german", "Alman Yerli", "Немецкая ландрас", "Німецький ландрас"],
  PIG_BLACK_PIED: ["Buntes Bentheimer Schwein", "Bentheim Black Pied", "Pie noir de Bentheim", "Bentheim Black Pied", "Bentheim Black Pied", "Pezzato nero Bentheim", "Bentheimer landschaap", "Bentheimské černostrakaté", "Porco colorido de Bentheim", "Színes Bentheimer", "Bentheim Black Pied", "Bentheim Black Pied", "Bentheim Black Pied", "Bentheim Alaca", "Бентгеймская черно-пестрая", "Бентхаймська сільська порода"],
  PIG_BERKSHIRE: ["Berkshire", "Berkshire", "Berkshire", "Berkshire", "Berkszyry", "Berkshire", "Berkshire", "Berkshirské", "Berkshire", "Berkshire", "Berkshire", "Berkshire", "Berkshire", "Berkshire", "Беркширская", "Беркширська порода"],
  CHICKEN: ["Huhn", "Chicken", "Poule", "Pollo", "Kura", "Gallina", "Kip", "Slepice", "Galinha", "Csirke", "Høne", "Höna", "Pui", "Tavuk", "Курица", "Курка"],
  CHICKEN_ROOSTER: ["Hahn", "Rooster", "Coq", "Gallo", "Kogut", "Gallo", "Haan", "Kohout", "Galo", "Kakas", "Hane", "Tupp", "Cocoș", "Horoz", "Петух", "Півень"],
  GOAT: ["Ziege", "Goat", "Chèvre", "Cabra", "Koza", "Capra", "Geit", "Koza", "Cabra", "Kecske", "Ged", "Get", "Capră", "Keçi", "Коза", "Коза"],
  ONION: ["Zwiebeln", "Onions", "Oignons", "Cebollas", "Cebula", "Cipolle", "Uien", "Cibule", "Cebolas", "Hagyma", "Løg", "Lökar", "Ceapă", "Soğanlar", "Лук", "Цибуля"],
  BANANA: ["Bananen", "Bananas", "Bananes", "Plátanos", "Banany", "Banane", "Bananen", "Banány", "Bananas", "Banán", "Bananer", "Bananer", "Banane", "Muz", "Бананы", "Банани"],
  COFFEE: ["Kaffee", "Coffee", "Café", "Café", "Kawa", "Caffè", "Koffie", "Káva", "Café", "Kávé", "Kaffe", "Kaffe", "Cafea", "Kahve", "Кофе", "Кава"],
  COFFEECHERRIES: ["Kaffeekirschen", "Coffee Cherries", "Cerises de café", "Cerezas de café", "Owoce kawy", "Ciliegie di caffè", "Koffiebessen", "Kávové třešně", "Cerejas de café", "Kávécseresznye", "Kaffebær", "Kaffebär", "Cireșe de cafea", "Kahve Kirazı", "Кофейные ягоды", "Кавові ягоди"],
  IRONORE: ["Eisenerz", "Iron Ore", "Minerai de fer", "Mineral de hierro", "Ruda żelaza", "Minerale di ferro", "IJzererts", "Železná ruda", "Minério de Ferro", "Vasérc", "Jernmalm", "Järnmalm", "Minereu de fier", "Demir Madeni", "Железная руда", "Залізна руда"],
  METAL: ["Metall", "Metal", "Métal", "Metal", "Metal", "Metallo", "Metaal", "Kov", "Metal", "Fém", "Metal", "Metall", "Metal", "Metal", "Металл", "Метал"],
  ARMOIRE: ["Schrank", "Armoire", "Armoire", "Armario", "Kredens", "Guardaroba", "Kast", "Skříň", "Armário", "Szekrény", "Klædeskab", "Klädskåp", "Șifonier", "Gardırop", "Гардероб", "Шафа"],
  BIRDHOUSE: ["Vogelhaus", "Bird House", "Nichoir", "Casa de pájaros", "Domek dla ptaków", "Casetta per uccelli", "Vogelhuis", "Ptačí budka", "Casa para Pássaros", "Madárház", "Fuglehus", "Fågelholk", "Căsuță pentru păsări", "Kuş Evi", "Скворечник", "Шпаківня"],
  BOWL: ["Schüssel", "Bowl", "Bol", "Cuenco", "Miska", "Ciotola", "Kom", "Miska", "Tigela", "Tál", "Skål", "Skål", "Bol", "Kap", "Миска", "Миска"],
  CATTREE: ["Kratzbaum", "Cat Tree", "Arbre à chat", "Árbol para gatos", "Drzewo dla kotów", "Tiragraffi", "Krabpaal", "Škrabadlo", "Trepador para Gatos", "Macskakaparó", "Kattetræ", "Klösträd", "Copac pentru pisici", "Kedi Evi", "Кошачья башня", "Котяча вежа"],
  CHAIR: ["Stuhl", "Chair", "Chaise", "Silla", "Krzesło", "Sedia", "Stoel", "Židle", "Cadeira", "Szék", "Stol", "Stol", "Scaun", "Sandalye", "Стул", "Стілець"],
  DOGHOUSE: ["Hundehütte", "Dog House", "Niche", "Caseta para perro", "Psia buda", "Cuccia del cane", "Hondenhok", "Psí bouda", "Casota", "Kutyaház", "Hundehus", "Hundkoja", "Cușcă pentru câini", "Köpek Kulübesi", "Собачья будка", "Собача буда"],
  EASEL: ["Staffelei", "Easel", "Chevalet", "Caballete", "Sztaluga", "Cavalletto", "Schildersezel", "Malířský stojan", "Cavalete", "Festőállvány", "Staffeli", "Staffli", "Șevalet", "Şövale", "Мольберт", "Мольберт"],
  FLOORTILES: ["Bodenfliesen", "Floor Tiles", "Dalles de sol", "Baldosas", "Płytki podłogowe", "Piastrelle", "Vloertegels", "Dlaždice", "Tacos de Piso", "Padlólap", "Gulvfliser", "Golvplattor", "Dușumea", "Yer Karoları", "Напольные плитки", "Плитка для підлоги"],
  PEPPERGRINDER: ["Pfeffermühle", "Pepper Grinder", "Moulin à poivre", "Molinillo de pimienta", "Młynek do pieprzu", "Macinapepe", "Pepermolen", "Mlýnek na pepř", "Moinho de Pimenta", "Borsdaráló", "Peberkværn", "Pepparkvarn", "Râșniță de piper", "Karabiber Öğütücüsü", "Перечница", "Млин для перцю"],
  PICTUREFRAME: ["Bilderrahmen", "Picture Frame", "Cadre photo", "Marco de fotos", "Rama", "Cornice", "Fotolijst", "Rám obrazu", "Moldura Fotográfica", "Képkeret", "Billedramme", "Ram", "Ramă pentru fotografie", "Resim Çerçevesi", "Рамы для картин", "Фоторамка"],
  SHINGLE: ["Schindel", "Shingle", "Bardeau", "Teja", "Dachówka", "Tegola", "Daklei", "Šindele", "Telha", "Zsindely", "Tagspån", "Takplattor", "Țiglă", "Çatı Kiremidi", "Черепица", "Дранка"],
  STAIRCASERAILING: ["Treppengeländer", "Staircase Railing", "Rampe d'escalier", "Barandilla", "Poręcz schodów", "Ringhiera per scala", "Trapleuning", "Zábradlí schodiště", "Corrimão de Escadaria", "Lépcsőkorlát", "Trappegelænder", "Trappräcke", "Balustradă pentru scări", "Merdiven Tırabzanı", "Лестничные перила", "Сходові поручні"],
  TABLE: ["Tisch", "Table", "Table", "Mesa", "Stół", "Tavolo", "Tafel", "Stůl", "Mesa", "Asztal", "Bord", "Bord", "Masă", "Tabla", "Стол", "Стіл"],
};

export const LANGUAGE_OPTIONS: ReadonlyArray<{
  code: SupportedLocale;
  shortLabel: string;
  nativeName: string;
}> = [
  { code: "de", shortLabel: "DE", nativeName: "Deutsch" },
  { code: "en", shortLabel: "EN", nativeName: "English" },
  { code: "fr", shortLabel: "FR", nativeName: "Français" },
  { code: "es", shortLabel: "ES", nativeName: "Español" },
  { code: "it", shortLabel: "IT", nativeName: "Italiano" },
  { code: "pl", shortLabel: "PL", nativeName: "Polski" },
  { code: "nl", shortLabel: "NL", nativeName: "Nederlands" },
  { code: "cz", shortLabel: "CZ", nativeName: "Čeština" },
  { code: "pt", shortLabel: "PT", nativeName: "Português" },
  { code: "hu", shortLabel: "HU", nativeName: "Magyar" },
  { code: "da", shortLabel: "DA", nativeName: "Dansk" },
  { code: "sv", shortLabel: "SV", nativeName: "Svenska" },
  { code: "ro", shortLabel: "RO", nativeName: "Română" },
  { code: "tr", shortLabel: "TR", nativeName: "Türkçe" },
  { code: "ru", shortLabel: "RU", nativeName: "Русский" },
  { code: "uk", shortLabel: "UK", nativeName: "Українська" },
];

let activeLocale: SupportedLocale = "de";

export function normalizeLocale(raw: string | null | undefined): SupportedLocale {
  const short = (raw ?? "en").trim().toLowerCase().split(/[-_]/)[0];
  switch (short) {
    case "de":
    case "en":
    case "fr":
    case "es":
    case "pl":
    case "it":
    case "nl":
    case "cz":
    case "pt":
    case "hu":
    case "da":
    case "sv":
    case "ro":
    case "tr":
    case "ru":
    case "uk":
      return short;
    case "cs":
      return "cz";
    case "br":
      return "pt";
    case "dk":
      return "da";
    case "se":
      return "sv";
    case "ua":
      return "uk";
    default:
      return "en";
  }
}

export function setLocale(raw: string): SupportedLocale {
  activeLocale = normalizeLocale(raw);
  document.documentElement.lang = activeLocale === "cz" ? "cs" : activeLocale;
  return activeLocale;
}

export function getLocale(): SupportedLocale {
  return activeLocale;
}

/**
 * Translates a key using the active locale with guaranteed fallback to English (`en`).
 * Supports `{placeholder}` variable interpolation.
 */
export function t(
  key: TranslationKey,
  vars?: Record<string, string | number>,
): string {
  const dict = DICTIONARIES[activeLocale] ?? DICTIONARIES.en;
  let template = dict[key] ?? DICTIONARIES.en[key] ?? key;

  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      template = template.replaceAll(`{${k}}`, String(v));
    }
  }
  return template;
}

/** Localizes a basegame FS25 fillType code across all 16 supported UI locales. */
export function localizeFillTypeCode(code: string): string | null {
  const row = BASEGAME_FILLTYPE_NAMES[code.trim().toUpperCase()];
  if (!row) return null;
  const idx = LOCALE_FILLTYPE_INDEX[activeLocale] ?? 1;
  return row[idx] ?? row[1] ?? null;
}

export function formatAccessTierName(tier: AccessTier | string): string {
  switch (tier) {
    case "developer":
      return t("tierNameDeveloper");
    case "extended":
      return t("tierNameExtended");
    default:
      return t("tierNameUser");
  }
}

/**
 * Renders a `LogEntry` in the currently selected UI language, re-evaluating crop codes
 * and tier labels dynamically whenever the user switches languages.
 */
export function formatLogEntryMessage(entry: LogEntry): string {
  if (!entry.key || !(entry.key in DICTIONARIES.en)) {
    return entry.message;
  }

  const vars: Record<string, string> = { ...(entry.params ?? {}) };

  if (vars.itemCode) {
    const localized = localizeFillTypeCode(vars.itemCode);
    if (localized) {
      vars.item = localized;
    }
  }
  if (vars.tier) {
    vars.tier = formatAccessTierName(vars.tier);
  }
  // Server notice times arrive as UTC unix seconds (`startUnix` / `endUnix`) and show in local time.
  for (const [unixKey, target] of [
    ["startUnix", "start"],
    ["endUnix", "end"],
  ] as const) {
    const secs = Number(vars[unixKey]);
    if (vars[unixKey] && Number.isFinite(secs) && secs > 0) {
      const locale = activeLocale === "cz" ? "cs" : activeLocale;
      try {
        vars[target] = new Date(secs * 1000).toLocaleString(locale, {
          weekday: "short",
          day: "2-digit",
          month: "2-digit",
          hour: "2-digit",
          minute: "2-digit",
        });
      } catch {
        vars[target] = new Date(secs * 1000).toLocaleString();
      }
    }
  }
  if (vars.market) {
    const mLower = vars.market.trim().toLowerCase();
    if (
      mLower === "global market" ||
      mLower === "global market official" ||
      mLower === "official global market"
    ) {
      vars.market = t("officialMarketName");
    } else if (mLower === "local simulation (mock)") {
      vars.market = t("marketNameMock");
    }
  }

  return t(entry.key as TranslationKey, vars);
}
