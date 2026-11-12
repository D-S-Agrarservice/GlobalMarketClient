//! Localized fallback display names for standard Farming Simulator 25 fillTypes (`DE`, `EN`, `FR`, `ES`, `PL`).
//!
//! When FS25 is actively connected, localized fillType titles are also learned directly from
//! `write/whitelist_*.xml` (covering custom Mod-Map crops automatically). This dictionary ensures
//! Great Demand OS desktop notifications and Mock simulation items always match the selected UI
//! language even before FS25 writes a whitelist or when switching languages on the fly.

/// Returns a localized display name for common FS25 basegame fillTypes in `de`, `en`, `fr`, `es`, or `pl`.
pub fn localize_fill_type(fill_type: &str, lang: &str) -> Option<&'static str> {
    let code = fill_type.trim().to_ascii_uppercase();
    let short_lang = lang
        .trim()
        .to_ascii_lowercase()
        .split(['-', '_'])
        .next()
        .unwrap_or("en")
        .to_string();

    // Tuple order: (DE, EN, FR, ES, PL)
    let names: (&'static str, &'static str, &'static str, &'static str, &'static str) =
        match code.as_str() {
            "WHEAT" => ("Weizen", "Wheat", "Blé", "Trigo", "Pszenica"),
            "BARLEY" => ("Gerste", "Barley", "Orge", "Cebada", "Jęczmień"),
            "OAT" => ("Hafer", "Oats", "Avoine", "Avena", "Owies"),
            "CANOLA" => ("Raps", "Canola", "Colza", "Colza", "Rzepak"),
            "SORGHUM" => ("Sorghumhirse", "Sorghum", "Sorgho", "Sorgo", "Sorgo"),
            "SUNFLOWER" => (
                "Sonnenblumen",
                "Sunflowers",
                "Tournesol",
                "Girasoles",
                "Słonecznik",
            ),
            "SOYBEAN" => ("Sojabohnen", "Soybeans", "Soja", "Soja", "Soja"),
            "CORN" | "MAIZE" => ("Mais", "Corn", "Maïs", "Maíz", "Kukurydza"),
            "POTATO" => (
                "Kartoffeln",
                "Potatoes",
                "Pommes de terre",
                "Patatas",
                "Ziemniaki",
            ),
            "SUGARBEET" => (
                "Zuckerrüben",
                "Sugar Beet",
                "Betterave sucrière",
                "Remolacha azucarera",
                "Buraki cukrowe",
            ),
            "SUGARCANE" => (
                "Zuckerrohr",
                "Sugarcane",
                "Canne à sucre",
                "Caña de azúcar",
                "Trzcina cukrowa",
            ),
            "COTTON" => ("Baumwolle", "Cotton", "Coton", "Algodón", "Bawełna"),
            "OLIVE" => ("Oliven", "Olives", "Olives", "Aceitunas", "Oliwki"),
            "GRAPE" => ("Trauben", "Grapes", "Raisins", "Uvas", "Winogrona"),
            "RICE" => ("Reis", "Rice", "Riz", "Arroz", "Ryż"),
            "LONGGRAINRICE" => (
                "Langkornreis",
                "Long Grain Rice",
                "Riz long grain",
                "Arroz de grano largo",
                "Ryż długoziarnisty",
            ),
            "PEAS" => ("Erbsen", "Peas", "Petits pois", "Guisantes", "Groch"),
            "SPINACH" => ("Spinat", "Spinach", "Épinards", "Espinacas", "Szpinak"),
            "GREENBEAN" => (
                "Grüne Bohnen",
                "Green Beans",
                "Haricots verts",
                "Judías verdes",
                "Fasolka szparagowa",
            ),
            "SILAGE" => ("Silage", "Silage", "Ensilage", "Ensilaje", "Kiszonka"),
            "STRAW" => ("Stroh", "Straw", "Paille", "Paja", "Słoma"),
            "DRYGRASS_WINDROW" | "HAY" => ("Heu", "Hay", "Foin", "Heno", "Siano"),
            "GRASS_WINDROW" | "GRASS" => ("Gras", "Grass", "Herbe", "Hierba", "Trawa"),
            "WOODCHIPS" => (
                "Hackschnitzel",
                "Wood Chips",
                "Copeaux de bois",
                "Astillas de madera",
                "Zrębki",
            ),
            "MILK" => ("Milch", "Milk", "Lait", "Leche", "Mleko"),
            "EGG" => ("Eier", "Eggs", "Œufs", "Huevos", "Jajka"),
            "WOOL" => ("Wolle", "Wool", "Laine", "Lana", "Wełna"),
            "HONEY" => ("Honig", "Honey", "Miel", "Miel", "Miód"),
            "FLOUR" => ("Mehl", "Flour", "Farine", "Harina", "Mąka"),
            "BREAD" => ("Brot", "Bread", "Pain", "Pan", "Chleb"),
            "SUGAR" => ("Zucker", "Sugar", "Sucre", "Azúcar", "Cukier"),
            "BOARDS" => ("Bretter", "Planks", "Planches", "Tablones", "Deski"),
            _ => return None,
        };

    Some(match short_lang.as_str() {
        "de" => names.0,
        "fr" => names.2,
        "es" => names.3,
        "pl" => names.4,
        _ => names.1,
    })
}
