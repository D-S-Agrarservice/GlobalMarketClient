import en from "./locales/en.json";
import de from "./locales/de.json";
import fr from "./locales/fr.json";
import es from "./locales/es.json";
import pl from "./locales/pl.json";
import type { AccessTier, LogEntry, SupportedLocale } from "./types";

export type TranslationKey = keyof typeof en;
type Dictionary = Record<TranslationKey, string>;

const DICTIONARIES: Record<SupportedLocale, Dictionary> = {
  en,
  de,
  fr,
  es,
  pl,
};

/**
 * 5-language fallback lookup for official FS25 basegame fillTypes (`[de, en, fr, es, pl]`).
 * Sourced from GIANTS `data/maps/maps_fillTypes.xml` + `l10n_{de,en,fr,es,pl}.xml` so historic
 * log lines, Great Demand badges, and Mock simulation items switch language dynamically live.
 */
const BASEGAME_FILLTYPE_NAMES: Record<
  string,
  readonly [string, string, string, string, string]
> = {
  WHEAT: ["Weizen", "Wheat", "Blé", "Trigo", "Pszenica"],
  BARLEY: ["Gerste", "Barley", "Orge", "Cebada", "Jęczmień"],
  OAT: ["Hafer", "Oat", "Avoine", "Avena", "Owies"],
  CANOLA: ["Raps", "Canola", "Colza", "Colza", "Rzepak"],
  SORGHUM: ["Hirse", "Sorghum", "Sorgho", "Sorgo", "Sorgo"],
  SUNFLOWER: ["Sonnenblumen", "Sunflowers", "Tournesols", "Girasoles", "Słoneczniki"],
  SOYBEAN: ["Sojabohnen", "Soybeans", "Soja", "Soja", "Soja"],
  MAIZE: ["Mais", "Corn", "Maïs", "Maíz", "Kukurydza"],
  CORN: ["Mais", "Corn", "Maïs", "Maíz", "Kukurydza"],
  POTATO: ["Kartoffeln", "Potatoes", "Pommes de terre", "Patatas", "Ziemniaki"],
  SUGARBEET: ["Zuckerrüben", "Sugar Beet", "Betteraves sucrières", "Remolacha azucarera", "Burak cukrowy"],
  BEETROOT: ["Rote Beete", "Red Beet", "Betterave", "Remolacha", "Burak czerwony"],
  CARROT: ["Karotten", "Carrots", "Carottes", "Zanahorias", "Marchew"],
  PARSNIP: ["Pastinaken", "Parsnips", "Panais", "Chirivía", "Pasternak"],
  COTTON: ["Baumwolle", "Cotton", "Coton", "Algodón", "Bawełna"],
  RICELONGGRAIN: ["Langkornreis", "Long Grain Rice", "Riz long grain", "Arroz de grano largo", "Ryż długoziarnisty"],
  LONGGRAINRICE: ["Langkornreis", "Long Grain Rice", "Riz long grain", "Arroz de grano largo", "Ryż długoziarnisty"],
  RICE: ["Reis", "Rice", "Riz", "Arroz", "Ryż"],
  GREENBEAN: ["Buschbohnen", "Green Beans", "Haricots verts", "Judías verdes", "Fasolka zielona"],
  PEA: ["Erbsen", "Peas", "Petits pois", "Guisantes", "Groszek"],
  PEAS: ["Erbsen", "Peas", "Petits pois", "Guisantes", "Groszek"],
  SPINACH: ["Spinat", "Spinach", "Épinards", "Espinacas", "Szpinak"],
  SUGARCANE: ["Zuckerrohr", "Sugarcane", "Canne à sucre", "Caña de azúcar", "Trzcina cukrowa"],
  WOOD: ["Holz", "Wood", "Bois", "Madera", "Drewno"],
  WOODCHIPS: ["Hackschnitzel", "Wood Chips", "Copeaux de bois", "Virutas", "Zrębki"],
  SUGARBEET_CUT: ["Zuckerrübenschnitzel", "Sugar Beet Cut", "Betteraves sucrières coupées", "Corte de remolacha azucarera", "Rozdrobnione buraki cukrowe"],
  SILAGE: ["Silage", "Silage", "Ensilage", "Ensilaje", "Kiszonka"],
  GRASS: ["Gras", "Grass", "Herbe", "Hierba", "Trawa"],
  MEADOW: ["Gras", "Grass", "Herbe", "Hierba", "Trawa"],
  GRASS_WINDROW: ["Gras", "Grass", "Herbe", "Hierba", "Trawa"],
  DRYGRASS: ["Gras", "Grass", "Herbe", "Hierba", "Trawa"],
  DRYGRASS_WINDROW: ["Heu", "Hay", "Foin", "Heno", "Siano"],
  HAY: ["Heu", "Hay", "Foin", "Heno", "Siano"],
  STRAW: ["Stroh", "Straw", "Paille", "Paja", "Słoma"],
  WHEAT_CUT: ["Weizen", "Wheat", "Blé", "Trigo", "Pszenica"],
  BARLEY_CUT: ["Gerste", "Barley", "Orge", "Cebada", "Jęczmień"],
  OAT_CUT: ["Hafer", "Oat", "Avoine", "Avena", "Owies"],
  CANOLA_CUT: ["Raps", "Canola", "Colza", "Colza", "Rzepak"],
  SOYBEAN_CUT: ["Sojabohnen", "Soybeans", "Soja", "Soja", "Soja"],
  GRAPE: ["Trauben", "Grapes", "Raisin", "Uvas", "Winogrona"],
  OLIVE: ["Oliven", "Olives", "Olives", "Aceitunas", "Oliwki"],
  MILK: ["Milch", "Milk", "Lait", "Leche", "Mleko"],
  GOATMILK: ["Ziegenmilch", "Goat Milk", "Lait de chèvre", "Leche de cabra", "Kozie mleko"],
  BUFFALOMILK: ["Büffelmilch", "Buffalo Milk", "Lait de bufflonne", "Leche de búfala", "Bawole mleko"],
  WOOL: ["Wolle", "Wool", "Laine", "Lana", "Wełna"],
  EGG: ["Eier", "Eggs", "Œufs", "Huevos", "Jaja"],
  LIQUIDSEEDTREATMENT: ["Beizmittel", "Seed Treating Liquid", "Liquide de traitement des semences", "Líquido para el tratamiento de semillas", "Płyn do zaprawiania nasion"],
  HONEY: ["Honig", "Honey", "Miel", "Miel", "Miód"],
  LETTUCE: ["Salat", "Lettuce", "Laitue", "Lechuga", "Sałata"],
  TOMATO: ["Tomaten", "Tomatoes", "Tomates", "Tomates", "Pomidory"],
  STRAWBERRY: ["Erdbeeren", "Strawberries", "Fraises", "Fresas", "Truskawki"],
  SPRING_ONION: ["Frühlingslauch", "Spring Onion", "Cébettes", "Cebolleta", "Zielona cebulka"],
  NAPACABBAGE: ["Chinakohl", "Napa Cabbage", "Chou chinois", "Col china", "Kapusta pekińska"],
  CHILLI: ["Chilischoten", "Chili Peppers", "Piments", "Guindillas", "Papryczki chili"],
  GARLIC: ["Knoblauch", "Garlic", "Ail", "Ajo", "Czosnek"],
  ENOKI: ["Enoki", "Enoki", "Enoki", "Enoki", "Enoki"],
  OYSTER: ["Austernpilz", "Oyster Mushroom", "Pleurote", "Champiñón ostra", "Boczniak ostrygowaty"],
  BOARDS: ["Bretter", "Planks", "Planches", "Tablas", "Deski"],
  PLANKS: ["Lange Bretter", "Planks Long", "Longues planches", "Tablas largas", "Długie deski"],
  WOODBEAM: ["Holzbalken", "Wood Beam", "Poutre en bois", "Viga de madera", "Drewniana belka"],
  PREFABWALL: ["Fertigwand", "Prefab Wall", "Mur préfabriqué", "Pared prefabricada", "Gotowa ściana"],
  FLOUR: ["Mehl", "Flour", "Farine", "Harina", "Mąka"],
  RICEFLOUR: ["Reismehl", "Rice Flour", "Farine de riz", "Harina de arroz", "Mąka ryżowa"],
  BREAD: ["Brot", "Bread", "Pain", "Pan", "Chleb"],
  CAKE: ["Kuchen", "Cake", "Gâteau", "Tarta", "Ciasto"],
  MILK_BOTTLED: ["Milch (abgefüllt)", "Milk (bottled)", "Lait (en bouteille)", "Leche (embotellada)", "Mleko w butelkach"],
  GOATMILK_BOTTLED: ["Ziegenmilch (abgefüllt)", "Goat Milk (bottled)", "Lait de chèvre (en bouteille)", "Leche de cabra (embotellada)", "Kozie mleko w butelkach"],
  BUFFALOMILK_BOTTLED: ["Büffelmilch (abgefüllt)", "Buffalo Milk (bottled)", "Lait de bufflonne (en bouteille)", "Leche de búfala (embotellada)", "Bawole mleko w butelkach"],
  BUTTER: ["Butter", "Butter", "Beurre", "Mantequilla", "Masło"],
  CHEESE: ["Käse", "Cheese", "Fromage", "Queso", "Ser żółty"],
  BUFFALOMOZZARELLA: ["Büffelmozzarella", "Buffalo Mozzarella", "Mozzarella de bufflonne", "Mozzarella de búfala", "Bawola mozzarella"],
  GOATCHEESE: ["Ziegenkäse", "Goat cheese", "Fromage de chèvre", "Queso de cabra", "Kozi ser"],
  CHOCOLATE: ["Schokolade", "Chocolate", "Chocolat", "Chocolate", "Czekolada"],
  POTATOCHIPS: ["Kartoffelchips", "Potato Chips", "Chips", "Patatas fritas", "Czipsy ziemniaczane"],
  RICEROLLS: ["Reisrollen", "Rice Rolls", "Galettes de riz soufflé", "Tortitas de arroz", "Sajgonki"],
  FERMENTEDNAPACABBAGE: ["Kimchi", "Kimchi", "Kimchi", "Kimchi", "Kimchi"],
  PRESERVEDCARROTS: ["Karottendosen", "Preserved Food Carrots", "Carottes en conserve", "Zanahoria en conserva", "Marchew konserwowa"],
  PRESERVEDPARSNIP: ["Pastinakendosen", "Preserved Food Parsnip", "Panais en conserve", "Chirivía en conserva", "Pasternak konserwowy"],
  PRESERVEDBEETROOT: ["Rote-Bete-Dosen", "Preserved Food Beetroot", "Betteraves en conserve", "Remolacha en conserva", "Buraki konserwowe"],
  NOODLESOUP: ["Nudelsuppe", "Noodle Soup", "Soupe de nouilles", "Sopa de fideos", "Zupa azjatycka"],
  SOUPCANSMIXED: ["Dreierlei-Suppe", "Triple Soup", "Soupe aux trois légumes", "Sopa triple", "Zupa z trzech składników"],
  SOUPCANSCARROTS: ["Karottensuppe", "Carrot Soup", "Soupe de carotte", "Sopa de zanahoria", "Zupa marchewkowa"],
  SOUPCANSPARSNIP: ["Pastinakensuppe", "Parsnip Soup", "Soupe de panais", "Sopa de chirivía", "Zupa z pasternaku"],
  SOUPCANSBEETROOT: ["Rote-Beete-Suppe", "Red Beet Soup", "Soupe de betterave", "Sopa de remolacha", "Barszcz czerwony"],
  SOUPCANSPOTATO: ["Kartoffelsuppe", "Potato Soup", "Soupe de pomme de terre", "Sopa de patata", "Zupa ziemniaczana"],
  FABRIC: ["Stoff", "Fabric", "Tissu", "Tejido", "Tkaniny"],
  CLOTHES: ["Kleidung", "Clothes", "Vêtements", "Ropa", "Ubrania"],
  SUGAR: ["Zucker", "Sugar", "Sucre", "Azúcar", "Cukier"],
  CEREAL: ["Müsli", "Cereal", "Céréales", "Cereales", "Płatki zbożowe"],
  FURNITURE: ["Möbel", "Furniture", "Mobilier", "Muebles", "Meble"],
  CARTONROLL: ["Kartonrolle", "Carton Roll", "Rouleau de carton", "Rollo de cartón", "Rolka tektury"],
  PAPERROLL: ["Papierrolle", "Paper Roll", "Rouleau de papier", "Rollo de papel", "Rolka papieru"],
  PIANO: ["Piano", "Piano", "Piano", "Piano", "Fortepian"],
  WAGON: ["Anhänger", "Wagon", "Wagon", "Remolque", "Wóz"],
  TOYTRACTOR: ["Spielzeugtraktor", "Toy Tractor", "Tracteur jouet", "Tractor de juguete", "Zabawkowy traktor"],
  SUNFLOWER_OIL: ["Sonnenblumenöl", "Sunflower Oil", "Huile de tournesol", "Aceite de girasol", "Olej słonecznikowy"],
  CANOLA_OIL: ["Rapsöl", "Canola Oil", "Huile de colza", "Aceite de colza", "Olej rzepakowy"],
  OLIVE_OIL: ["Olivenöl", "Olive Oil", "Huile d'olive", "Aceite de oliva", "Oliwa z oliwek"],
  RICE_OIL: ["Reisöl", "Rice Oil", "Huile de riz", "Aceite de arroz", "Olej ryżowy"],
  RAISINS: ["Rosinen", "Raisins", "Raisins secs", "Pasas", "Rodzynki"],
  GRAPEJUICE: ["Traubensaft", "Grape Juice", "Jus de raisin", "Zumo de uva", "Sok z winogron"],
  CEMENT: ["Zementsäcke", "Cement bags", "Sacs de ciment", "Sacos de cemento", "Worki z cementem"],
  BARREL: ["Fass", "Barrel", "Tonneau", "Barril", "Beczka"],
  BATHTUB: ["Badewanne", "Bathtub", "Baignoire", "Bañera", "Wanna"],
  BUCKET: ["Eimer", "Bucket", "Seau", "Cubo", "Wiadro"],
  ROOFPLATES: ["Dachplatte", "Roof Plate", "Tôle de toiture", "Chapa de tejado", "Pokrycie dachowe"],
  ROPE: ["Seil", "Rope", "Corde", "Cuerda", "Sznur"],
  CANNED_PEAS: ["Dosenerbsen", "Canned Peas", "Petit pois en conserve", "Guisantes en conserva", "Puszkowany groszek"],
  CEMENTBRICKS: ["Betonziegel", "Cement Brick", "Briques en ciment", "Ladrillo de cemento", "Cementowa cegła"],
  SPINACH_BAGS: ["Spinatsack", "Spinach Bag", "Sac d'épinards", "Saco de espinacas", "Worek szpinaku"],
  JARRED_GREENBEAN: ["Eingemachte Buschbohnen", "Jarred Green Bean", "Bocal de haricots verts", "Judías verdes en frasco", "Słoikowana fasolka zielona"],
  RICE_BAGS: ["Reissack", "Rice Bag", "Sac de riz", "Saco de arroz", "Worek ryżu"],
  RICE_BOXES: ["Reisbox", "Rice Box", "Boîte de riz", "Caja de arroz", "Pudełko ryżu"],
  SEEDS: ["Saatgut", "Seeds", "Semences", "Semillas", "Nasiona"],
  STONE: ["Steine", "Stones", "Pierres", "Piedras", "Kamienie"],
  MANURE: ["Mist", "Manure", "Fumier", "Estiércol", "Obornik"],
  LIQUIDMANURE: ["Gülle", "Slurry", "Lisier", "Purín", "Gnojówka"],
  DIGESTATE: ["Gärreste", "Digestate", "Digestat", "Digestato", "Poferment"],
  FORAGE: ["Totalmischration", "Total Mixed Ration", "Ration totale mélangée", "Ración mezclada", "Pasza TMR"],
  FORAGE_MIXING: ["Futter", "Forage", "Fourrage", "Forraje", "Pasza"],
  WATER: ["Wasser", "Water", "Eau", "Agua", "Woda"],
  CHAFF: ["Häckselgut", "Chaff", "Paille hachée", "Paja fina", "Sieczka"],
  TREESAPLINGS: ["Setzlinge", "Tree Saplings", "Plants d'arbres", "Plantones", "Sadzonki"],
  TREE: ["Baum", "Tree", "Arbre", "Árbol", "Drzewo"],
  OILSEEDRADISH: ["Ölrettich", "Oilseed Radish", "Radis oléagineux", "Rábano oleífero", "Rzodkiew oleista"],
  POPLAR: ["Pappel", "Poplar", "Peuplier", "Álamo", "Topola"],
  DIESEL: ["Diesel", "Diesel", "Diesel", "Diésel", "Olej napędowy"],
  DEF: ["Synthetische Harnstofflösung", "Diesel exhaust fluid", "Fluide d'échappement diesel", "Fluido de escape de diésel", "Wodny roztwór mocznika"],
  AIR: ["Luft", "Air", "Air", "Aire", "Powietrze"],
  ELECTRICCHARGE: ["Elektrische Ladung", "Electric Charge", "Recharge électrique", "Carga eléctrica", "Ładowanie elektryczne"],
  METHANE: ["Methan", "Methane", "Méthane", "Metano", "Metan"],
  SNOW: ["Schnee", "Snow", "Neige", "Nieve", "Śnieg"],
  ROADSALT: ["Streusalz", "Road Salt", "Sel de déneigement", "Sal para carreteras", "Sól drogowa"],
  ROUNDBALE: ["Rundballen", "Round bale", "Balle ronde", "Paca redonda", "Bela"],
  ROUNDBALE_GRASS: ["Rundballen (Gras)", "Round bale grass", "Balle d'herbe ronde", "Hierba para pacas redondas", "Bela trawy"],
  ROUNDBALE_DRYGRASS: ["Rundballen (Heu)", "Round bale hay", "Balle de foin ronde", "Heno para pacas redondas", "Bela siana"],
  ROUNDBALE_COTTON: ["Rundballen (Baumwolle)", "Round bale cotton", "Balle de coton ronde", "Algodón para pacas redondas", "Bela bawełny"],
  ROUNDBALE_WOOD: ["Rundballen (Holz)", "Round bale wood", "Balle de bois ronde", "Madera para pacas redondas", "Bela drewna"],
  SQUAREBALE: ["Quaderballen", "Square bale", "Balle carrée", "Paca rectangular", "Kostka"],
  SQUAREBALE_GRASS: ["Quaderballen (Gras)", "Square bale grass", "Balle d'herbe carrée", "Hierba para pacas rectangulares", "Kostka trawy"],
  SQUAREBALE_DRYGRASS: ["Quaderballen (Heu)", "Square bale hay", "Balle de foin carrée", "Heno para pacas rectangulares", "Kostka siana"],
  SQUAREBALE_COTTON: ["Quaderballen (Baumwolle)", "Square bale cotton", "Balle de coton carrée", "Algodón para pacas rectangulares", "Kostka bawełny"],
  SQUAREBALE_WOOD: ["Quaderballen (Holz)", "Square bale wood", "Balle de bois carrée", "Madera para pacas rectangulares", "Kostka drewna"],
  BALE_WRAP: ["Wickelfolie", "Bale Wrap", "Film à balles", "Encintado de pacas", "Folia do owijania"],
  BALE_NET: ["Rundballennetz", "Round bale Net", "Filet à balles rondes", "Malla para pacas redondas", "Siatka do okragłych bel"],
  BALE_TWINE: ["Bindegarn für Quaderballen", "Square bale Twine", "Ficelle à balles carrées", "Cordel para pacas rectangulares", "Sznurek do kostek"],
  RICESAPLINGS: ["Reis Setzlinge", "Rice Saplings", "Plants de riz", "Plantones de arroz", "Sadzonki ryżu"],
  FERTILIZER: ["Mineraldünger", "Solid Fertilizer", "Engrais solide", "Abonadora", "Nawóz stały"],
  LIQUIDFERTILIZER: ["Flüssigdünger", "Liquid Fertilizer", "Engrais liquide", "Abono líquido", "Płynny nawóz"],
  PIGFOOD: ["Schweinefutter", "Pig Food", "Nourriture pour cochons", "Comida para cerdos", "Karma dla świń"],
  TARP: ["Abdeckplane", "Tarp", "Bâche", "Lona", "Plandeka"],
  LIME: ["Kalk", "Lime", "Chaux", "Cal", "Wapno"],
  HERBICIDE: ["Herbizid", "Herbicide", "Herbicide", "Herbicida", "Herbicyd"],
  SILAGE_ADDITIVE: ["Siliermittel", "Silage Additive", "Additifs d'ensilage", "Aditivo de ensilaje", "Dodatek do kiszonek"],
  MINERAL_FEED: ["Mineralfutter", "Mineral Feed", "Complément minéral", "Pienso mineral", "Pasza mineralna"],
  WEED: ["Unkraut", "Weed", "Mauvaises herbes", "Hierba", "Chwasty"],
  HORSE_PALOMINO: ["Isabell", "Palomino", "Cheval (palomino)", "Palomino", "Koń (izabelowaty)"],
  HORSE_BLACK: ["Rappe", "Black", "Cheval (noir)", "Negro", "Koń (kary)"],
  HORSE_BAY: ["Brauner", "Bay", "Cheval (bai)", "Castaño", "Koń (gniady)"],
  HORSE_PINTO: ["Schecke", "Pinto", "Cheval (pie)", "Pío", "Koń (gniadosrokaty)"],
  HORSE_SEAL_BROWN: ["Schwarzbrauner", "Seal brown", "Cheval (bai-brun)", "Pardo", "Koń (ciemnogniady)"],
  HORSE_GRAY: ["Schimmel", "Gray", "Cheval (gris)", "Tordo", "Koń (siwy)"],
  HORSE_DUN: ["Falbe", "Dun", "Cheval (isabelle)", "Dun", "Koń (bułany)"],
  HORSE_CHESTNUT: ["Fuchs", "Chestnut", "Cheval (alezan)", "Alazán", "Koń (kasztanowaty)"],
  COW_SWISS_BROWN: ["Brown-Swiss", "Brown-Swiss", "Brun noir", "Parda suiza", "Brown Swiss"],
  COW_HOLSTEIN: ["Holstein", "Holstein", "Holstein", "Holstein", "Holsztyno-fryzyjska"],
  COW_LIMOUSIN: ["Limousin", "Limousin", "Limousine", "Limousin", "Limousine"],
  COW_ANGUS: ["Angus", "Angus", "Angus", "Angus", "Angus"],
  COW_WATERBUFFALO: ["Wasserbüffel", "Water Buffalo", "Buffle", "Búfalo de agua", "Wół domowy"],
  COW_HIGHLAND_CATTLE: ["Hochlandrind", "Highland Cattle", "Bétail des Highlands", "Ganado de las tierras altas", "Szkocka rasa wyżynna"],
  COW_BRAHMAN: ["Brahman", "Brahman", "Brahmane", "Brahman", "Brahman"],
  SHEEP_LANDRACE: ["Bentheimer Landschaf", "Landrace of Bentheim", "Bentheim", "Landrace de Bentheim", "Landrace z Bentheim"],
  SHEEP_SWISS_MOUNTAIN: ["Schwarzbraunes Bergschaf", "Swiss Black-Brown Mountain", "Brun noir du Jura", "Marrón de las montañas de Gales", "Szwajcarska czarno-brązowa górska"],
  SHEEP_STEINSCHAF: ["Alpines Steinschaf", "Steinschaf", "Mouton des Alpes", "Steinschaf", "Steinschaf"],
  SHEEP_BLACK_WELSH: ["Black Welsh Mountain", "Black Welsh Mountain", "Mouton de montagne noir gallois", "Negra de las montañas de Gales", "Walijska górska"],
  PIG_LANDRACE: ["Deutsches Edelschwein", "German Landrace", "Landrace allemand", "German Landrace", "Niemiecka rasa krajowa"],
  PIG_BLACK_PIED: ["Buntes Bentheimer Schwein", "Bentheim Black Pied", "Pie noir de Bentheim", "Bentheim Black Pied", "Bentheim Black Pied"],
  PIG_BERKSHIRE: ["Berkshire", "Berkshire", "Berkshire", "Berkshire", "Berkszyry"],
  CHICKEN: ["Huhn", "Chicken", "Poule", "Pollo", "Kura"],
  CHICKEN_ROOSTER: ["Hahn", "Rooster", "Coq", "Gallo", "Kogut"],
  GOAT: ["Ziege", "Goat", "Chèvre", "Cabra", "Koza"],
  ONION: ["Zwiebeln", "Onions", "Oignons", "Cebollas", "Cebula"],
  BANANA: ["Bananen", "Bananas", "Bananes", "Plátanos", "Banany"],
  COFFEE: ["Kaffee", "Coffee", "Café", "Café", "Kawa"],
  COFFEECHERRIES: ["Kaffeekirschen", "Coffee Cherries", "Cerises de café", "Cerezas de café", "Owoce kawy"],
  IRONORE: ["Eisenerz", "Iron Ore", "Minerai de fer", "Mineral de hierro", "Ruda żelaza"],
  METAL: ["Metall", "Metal", "Métal", "Metal", "Metal"],
  ARMOIRE: ["Schrank", "Armoire", "Armoire", "Armario", "Kredens"],
  BIRDHOUSE: ["Vogelhaus", "Bird House", "Nichoir", "Casa de pájaros", "Domek dla ptaków"],
  BOWL: ["Schüssel", "Bowl", "Bol", "Cuenco", "Miska"],
  CATTREE: ["Kratzbaum", "Cat Tree", "Arbre à chat", "Árbol para gatos", "Drzewo dla kotów"],
  CHAIR: ["Stuhl", "Chair", "Chaise", "Silla", "Krzesło"],
  DOGHOUSE: ["Hundehütte", "Dog House", "Niche", "Caseta para perro", "Psia buda"],
  EASEL: ["Staffelei", "Easel", "Chevalet", "Caballete", "Sztaluga"],
  FLOORTILES: ["Bodenfliesen", "Floor Tiles", "Dalles de sol", "Baldosas", "Płytki podłogowe"],
  PEPPERGRINDER: ["Pfeffermühle", "Pepper Grinder", "Moulin à poivre", "Molinillo de pimienta", "Młynek do pieprzu"],
  PICTUREFRAME: ["Bilderrahmen", "Picture Frame", "Cadre photo", "Marco de fotos", "Rama"],
  SHINGLE: ["Schindel", "Shingle", "Bardeau", "Teja", "Dachówka"],
  STAIRCASERAILING: ["Treppengeländer", "Staircase Railing", "Rampe d'escalier", "Barandilla", "Poręcz schodów"],
  TABLE: ["Tisch", "Table", "Table", "Mesa", "Stół"],
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
  { code: "pl", shortLabel: "PL", nativeName: "Polski" },
];

let activeLocale: SupportedLocale = "de";

export function normalizeLocale(raw: string | null | undefined): SupportedLocale {
  const short = (raw ?? "en").trim().toLowerCase().split(/[-_]/)[0];
  if (short === "de" || short === "en" || short === "fr" || short === "es" || short === "pl") {
    return short;
  }
  return "en";
}

export function setLocale(raw: string): SupportedLocale {
  activeLocale = normalizeLocale(raw);
  document.documentElement.lang = activeLocale;
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

/** Localizes a basegame FS25 fillType code (`WHEAT` -> `Weizen`/`Wheat`/`Blé`/`Trigo`/`Pszenica`). */
export function localizeFillTypeCode(code: string): string | null {
  const row = BASEGAME_FILLTYPE_NAMES[code.trim().toUpperCase()];
  if (!row) return null;
  switch (activeLocale) {
    case "de":
      return row[0];
    case "fr":
      return row[2];
    case "es":
      return row[3];
    case "pl":
      return row[4];
    default:
      return row[1];
  }
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

  return t(entry.key as TranslationKey, vars);
}
