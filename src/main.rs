// pyroclear — terminal goes up in flames, then clears.
//
// Doom-fire algorithm: a heat grid (0..=36) where the bottom row is
// the fire source. Each frame, every cell pulls its new value from
// the cell below (with a bit of horizontal drift), minus random
// decay, so heat rises and cools. Heat → color via a palette.
// Whole frame is redrawn each tick via ANSI cursor-home, so no
// flicker and no need for a TUI crate.
//
// Terminal size is read via ioctl(TIOCGWINSZ) every frame (cheap
// syscall, no subprocess spawn), so the grid follows you live if
// you resize mid-burn. Randomness is a tiny xorshift64* PRNG seeded
// off the clock — no `rand` crate needed.
//
// COLOR SELECTION
//   pyroclear --color ice|toxic|purple|mono|plasma|sunset|ocean|lava|fire
//   pyroclear --from "#1a0000" --to "#ffcc00"    (custom gradient)
//   pyroclear --list-colors                      (colored swatches)
//   pyroclear --pick / -p                        (interactive TUI picker)
// Whatever you pick is written to ~/.config/pyroclear/config.toml
// and reused automatically on future runs until you change it again.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ESC: &str = "\x1b";
const MAX_HEAT: u8 = 36;

// Sim/render tuning
const FRAME_DELAY: Duration = Duration::from_millis(16); // ~60fps
const STEPS_PER_FRAME: u32 = 2; // extra propagation steps per frame
const MAX_DURATION: Duration = Duration::from_millis(2200);
const SOURCE_COOL_START: f32 = 0.38;
const DIE_OUT_THRESHOLD: u8 = 2;

// ---------------------------------------------------------------------
// Palettes
// ---------------------------------------------------------------------

type Palette = [(u8, u8, u8); 37];

// Punchy rework of the classic Doom fire ramp. The original values
// (from Fabian Sanglard's write-up) are intentionally muted — these
// push saturation in the red-to-orange band for a fiercer look.
// (Final on-screen look is softened/brightened by `soften()` below.)
const FIRE_PALETTE: Palette = [
    (0x08, 0x00, 0x00), (0x28, 0x02, 0x00), (0x3E, 0x08, 0x00), (0x56, 0x0A, 0x00),
    (0x6E, 0x0C, 0x00), (0x88, 0x10, 0x00), (0x9C, 0x14, 0x00), (0xB2, 0x1A, 0x00),
    (0xC4, 0x20, 0x00), (0xD4, 0x2C, 0x00), (0xE0, 0x38, 0x00), (0xE8, 0x44, 0x00),
    (0xF0, 0x50, 0x00), (0xF4, 0x5A, 0x00), (0xF6, 0x62, 0x00), (0xF6, 0x6A, 0x02),
    (0xF4, 0x72, 0x04), (0xF2, 0x7C, 0x06), (0xEE, 0x86, 0x08), (0xEA, 0x90, 0x0A),
    (0xE6, 0x9A, 0x0C), (0xE2, 0xA4, 0x0E), (0xDC, 0xAE, 0x10), (0xD8, 0xB8, 0x14),
    (0xD4, 0xC0, 0x18), (0xD0, 0xC8, 0x1C), (0xCC, 0xD0, 0x20), (0xD4, 0xD6, 0x38),
    (0xDC, 0xDC, 0x54), (0xE4, 0xE4, 0x74), (0xEA, 0xEA, 0x94), (0xF0, 0xF0, 0xB0),
    (0xF4, 0xF4, 0xC8), (0xF8, 0xF6, 0xDC), (0xFA, 0xFA, 0xEC), (0xFC, 0xFC, 0xF6),
    (0xFF, 0xFF, 0xFF),
];

// All named palettes: (id, display-name, description, from-hex, to-hex).
// "fire" is handled specially (uses FIRE_PALETTE above).

const NAMED_PALETTES: &[(&str, &str, &str, &str, &str)] = &[
    // --- original set, now vivid throughout (fire stays classic) ---
    ("fire",             "Fire",             "classic ember red → orange → white",        "#800000",  "#ffffff" ),
    ("ice",              "Ice",              "electric blue → brilliant white-cyan",     "#0040ff",  "#c8ffff" ),
    ("toxic",            "Toxic",            "radioactive green → acid lime",            "#00a020",  "#ccff33" ),
    ("purple",           "Purple",           "deep violet → hot lavender",               "#6a00c8",  "#e060ff" ),
    ("plasma",           "Plasma",           "electric violet → magenta white-hot",      "#7000ff",  "#ff60ff" ),
    ("sunset",           "Sunset",           "royal blue → blazing gold",                "#1a20c0",  "#ffb000" ),
    ("ocean",            "Ocean",            "deep sapphire → bright aqua",               "#0030a0",  "#00f0ff" ),
    ("lava",             "Lava",             "molten red → neon orange",                 "#e00010",  "#ff5500" ),
    ("mono",             "Mono",             "charcoal grey → pure white",               "#404040",  "#ffffff" ),
    ("gold",             "Gold",             "deep amber → bright gold",                 "#c07000",  "#ffe000" ),
    ("crimson",          "Crimson",          "blood red → hot pink",                     "#c00020",  "#ff2d80" ),
    ("emerald",          "Emerald",          "rich green → bright emerald",              "#009040",  "#40ffb0" ),
    ("mint",             "Mint",             "vivid teal → pale mint",                   "#00a880",  "#b0ffe8" ),
    ("rose",             "Rose",             "deep rose red → blush white-pink",         "#c02050",  "#ffd6e8" ),
    ("coral",            "Coral",            "vivid crimson → peach coral",              "#e02818",  "#ffb090" ),
    ("cobalt",           "Cobalt",           "electric cobalt → sky white",              "#0040d0",  "#d0f0ff" ),
    ("indigo",           "Indigo",           "deep indigo → periwinkle",                 "#3000c0",  "#a8a0ff" ),
    ("cyanpunk",         "Cyanpunk",         "deep cyan → neon white-cyan",              "#00a0a0",  "#40ffff" ),
    ("copper",           "Copper",           "burnt copper → rose gold",                 "#c04a10",  "#ffb070" ),
    ("steel",            "Steel",            "slate blue → bright silver",               "#405070",  "#e0e8f0" ),
    ("arctic",           "Arctic",           "glacier blue → pure white",                "#1080d0",  "#f0f8ff" ),
    ("volcano",          "Volcano",          "magma red-orange → cinder white",          "#d02800",  "#fff0e0" ),
    ("candy",            "Candy",            "hot magenta → cotton candy pink",          "#e000a0",  "#ffc0f0" ),
    ("midnight",         "Midnight",         "electric blue → starlight white",          "#0020c0",  "#e8e8ff" ),
    ("jade",             "Jade",             "vivid jade green → pale seafoam",          "#00a070",  "#c0ffe8" ),
    ("blood",            "Blood",            "deep blood red → pale ash pink",           "#a00010",  "#f0d0d0" ),
    ("dawn",             "Dawn",             "royal blue → warm cream-gold",             "#2020d0",  "#ffe0a0" ),
    ("void",             "Void",             "deep violet → pale lilac",                 "#4000a0",  "#e0d0ff" ),
    ("aurora",           "Aurora",           "teal glow → violet shimmer",               "#00b8a0",  "#8040ff" ),
    ("flamingo",         "Flamingo",         "hot pink → pale butter yellow",          "#ff2f92",  "#fff2b0" ),
    ("citrus",           "Citrus",           "lime zest → orange peel",                "#aaff20",  "#ff8c1a" ),
    ("bruise",           "Bruise",           "deep purple → sickly yellow-green",      "#3a0060",  "#9acc30" ),
    ("neon80s",          "Neon 80s",         "electric magenta → laser cyan",          "#ff20c0",  "#20e0ff" ),
    ("desertglow",       "Desert Glow",      "warm sand → burnt orange",               "#e8c080",  "#e0501a" ),
    ("glacierfire",      "Glacier Fire",     "ice blue → flame orange",                "#40c8ff",  "#ff6a20" ),
    ("venom",            "Venom",            "dark viper green → acid yellow",         "#104a20",  "#d0ff30" ),
    ("orchid",           "Orchid",           "deep magenta → soft blush pink",         "#a0006a",  "#ffc0e0" ),
    ("tropical",         "Tropical",         "turquoise water → coral reef",           "#10d0c0",  "#ff7a5c" ),
    ("wildfire",         "Wildfire",         "crimson blaze → golden ember",           "#c8102e",  "#ffcc33" ),
    ("galaxy",           "Galaxy",           "deep indigo → cosmic pink",              "#1a0060",  "#ff60c0" ),
    ("seafoam",          "Seafoam",          "deep teal → pale mint foam",             "#004a4a",  "#c0fff0" ),
    ("bubblegum",        "Bubblegum",        "grape purple → bubblegum pink",          "#7a1aff",  "#ff9ad8" ),
    ("solarflare",       "Solar Flare",      "burnt orange → white-hot yellow",        "#ff5a00",  "#fff8c0" ),
    ("abyss",            "Abyss",            "midnight navy → deep sea teal",          "#000428",  "#0a6060" ),
    ("rust",             "Rust",             "iron oxide brown → sandy tan",           "#7a2c10",  "#d8a870" ),
    ("cherry",           "Cherry",           "dark cherry red → soft pink blush",      "#6a0018",  "#ff9ab0" ),
    ("lagoon",           "Lagoon",           "ocean blue → jungle green",              "#0050a0",  "#20c060" ),
    ("peacock",          "Peacock",          "deep teal → royal purple",               "#005050",  "#8020c0" ),
    ("mango",            "Mango",            "golden orange → tropical yellow",        "#ff8c00",  "#fff060" ),
    ("grape",            "Grape",            "deep grape purple → light lilac",        "#4a0080",  "#d0b0ff" ),
    ("gasflame",         "Gas Flame",        "cool blue flame → white heat",           "#0050ff",  "#f0f8ff" ),
    ("sludge",           "Sludge",           "murky olive → radioactive yellow",       "#3a3a10",  "#e0ff20" ),
    ("rosegold",         "Rose Gold",        "dusty rose → warm gold",                 "#b76e79",  "#ffd7a0" ),
    ("spectrum",         "Spectrum",         "crimson red → deep sky blue",            "#ff1a3c",  "#1a8cff" ),
    ("blush",            "Blush",            "soft coral → pale lavender",             "#ff8a80",  "#d8c0ff" ),
    ("citrine",          "Citrine",          "amber yellow → burnt caramel",           "#ffd700",  "#a85a20" ),
    ("permafrost",       "Permafrost",       "deep glacier blue → pale ice cyan",      "#002a5a",  "#c0f0ff" ),
    ("amethyst",         "Amethyst",         "deep violet gem → soft lilac",           "#4a0e8f",  "#c8a0ff" ),
    ("saffron",          "Saffron",          "saffron yellow → deep crimson",          "#ffb300",  "#a4001e" ),
    ("bioluminescence",  "Bioluminescence",  "deep ocean teal → glowing green",        "#002030",  "#40ff90" ),
    ("dragonfruit",      "Dragonfruit",      "hot pink → lime green",                  "#ff2d6a",  "#a0ff30" ),
    ("obsidianember",    "Obsidian Ember",   "near-black red → bright ember orange",   "#1a0500",  "#ff7a1a" ),
    ("lilacmist",        "Lilac Mist",       "soft lilac → deep plum",                 "#d0b0ff",  "#5a1080" ),
    ("tigerlily",        "Tiger Lily",       "vivid orange → deep magenta",            "#ff7000",  "#c0006a" ),
    ("frostbite",        "Frostbite",        "icy white-blue → deep navy",             "#d0f0ff",  "#001040" ),
    ("molten",           "Molten",           "dark charcoal → bright molten gold",     "#201810",  "#ffcc00" ),
    ("nebula",           "Nebula",           "deep violet → pink-orange glow",         "#2a0060",  "#ff8060" ),
    ("kryptonite",       "Kryptonite",       "dark green → radioactive lime",          "#103010",  "#baff20" ),
    ("sakura",           "Sakura",           "deep rose → pale pink blossom",          "#c04070",  "#ffe0f0" ),
    ("inferno",          "Inferno",          "dark maroon → bright yellow",            "#400000",  "#ffe000" ),
    ("glacierpeak",      "Glacier Peak",     "deep teal-blue → pure white",            "#003050",  "#ffffff" ),
    ("voltage",          "Voltage",          "electric yellow → deep blue",            "#fff020",  "#1030c0" ),
    ("mermaid",          "Mermaid",          "deep teal → shimmering aqua",            "#004040",  "#40f0d0" ),
    ("rubygem",          "Ruby Gem",         "deep ruby red → bright pink",            "#700018",  "#ff4070" ),
    ("sunburst",         "Sunburst",         "deep orange → pale yellow",              "#ff6000",  "#fff8b0" ),
    ("twilight",         "Twilight",         "deep indigo → orange glow",              "#200040",  "#ff9040" ),
    ("absinthe",         "Absinthe",         "dark green → pale green-yellow",         "#103018",  "#d0ff90" ),
    ("plumfire",         "Plum Fire",        "deep plum → bright orange",              "#400030",  "#ff6020" ),
    ("iceberg",          "Iceberg",          "pale cyan → deep blue",                  "#b0f0ff",  "#002060" ),
    ("terracotta",       "Terracotta",       "burnt clay → warm cream",                "#a04020",  "#ffe0b0" ),
    ("neonjungle",       "Neon Jungle",      "deep jungle green → hot pink",           "#002810",  "#ff2090" ),
    ("copperpatina",     "Copper Patina",    "copper brown → teal-green",              "#a05020",  "#20a080" ),
    ("blacklight",       "Blacklight",       "deep purple-black → neon violet",        "#100020",  "#b040ff" ),
    ("sangria",          "Sangria",          "deep wine red → soft pink",              "#500018",  "#ffb0c0" ),
    ("sapphire",         "Sapphire",         "deep sapphire blue → pale sky",          "#002080",  "#a0e0ff" ),
    ("mangotango",       "Mango Tango",      "deep red-orange → bright mango yellow",  "#d02000",  "#ffcc30" ),
    ("periwinkle",       "Periwinkle",       "deep blue-violet → pale periwinkle",     "#3020a0",  "#c0c0ff" ),
    ("embertwilight",    "Ember Twilight",   "deep navy → ember orange",               "#000830",  "#ff5a1a" ),
    ("algae",            "Algae",            "dark murky green → bright chartreuse",   "#102010",  "#c0ff40" ),
    ("flamingosunset",   "Flamingo Sunset",  "deep magenta → golden yellow",           "#c00060",  "#ffd000" ),
    ("deepspace",        "Deep Space",       "true black-blue → starlight blue-white", "#000018",  "#c0d0ff" ),
    ("honeycomb",        "Honeycomb",        "deep amber-brown → bright honey gold",   "#603010",  "#ffcc40" ),
    ("vipergreen",       "Viper Green",      "dark green-black → bright toxic green",  "#081408",  "#90ff40" ),
    ("bloodmoon",        "Blood Moon",       "deep black-red → pale orange-red",       "#180000",  "#ff8060" ),
    ("cottoncandy",      "Cotton Candy",     "soft sky blue → soft pink",              "#a0d0ff",  "#ffb0e0" ),
    ("magmacore",        "Magma Core",       "dark red-black → bright yellow-orange",  "#200000",  "#ffaa00" ),
    ("deepteal",         "Deep Teal",        "near-black teal → pale seafoam white",   "#001818",  "#e0fff8" ),
    ("royale",           "Royale",           "deep royal purple → gold",               "#300060",  "#ffd700" ),
    ("cyberpink",        "Cyberpink",        "deep black-magenta → neon pink",         "#180010",  "#ff30a0" ),
    ("autumnleaf",       "Autumn Leaf",      "deep brown-red → golden orange",         "#601810",  "#ff9020" ),
    ("moonstone",        "Moonstone",        "pale grey-blue → deep indigo",           "#d0d8ff",  "#201060" ),
    ("hazard",           "Hazard",           "near-black → hazard yellow",             "#101008",  "#fff000" ),
    ("crimsontide",      "Crimson Tide",     "deep crimson → pale foam white",         "#600010",  "#fff0f0" ),

    // --- round 3: 200 more, dark → vivid, organized by hue family ---

    // reds
    ("scarlet",          "Scarlet",          "near-black → blazing scarlet",         "#1a0000",  "#ff2020" ),
    ("vermilion",        "Vermilion",        "charcoal red → vermilion orange-red",  "#200400",  "#ff5030" ),
    ("garnet",           "Garnet",           "dark garnet → bright rose-red",        "#240008",  "#e8305a" ),
    ("brickred",         "Brick Red",        "dark brick → warm terracotta red",     "#1c0806",  "#d85030" ),
    ("maroonglow",       "Maroon Glow",      "deep maroon → bright pink-red",        "#180008",  "#c02050" ),
    ("rubyflame",        "Ruby Flame",       "near-black → ruby red",                "#1a0004",  "#ff1050" ),
    ("carmine",          "Carmine",          "dark carmine → vivid rose",            "#1e0006",  "#ff2050" ),
    ("redwood",          "Redwood",          "dark redwood bark → warm red-orange",  "#1a0a04",  "#d86030" ),
    ("poppy",            "Poppy",            "near-black → poppy red",               "#200000",  "#ff3018" ),
    ("paprika",          "Paprika",          "dark spice → bright paprika red",      "#240800",  "#e85018" ),
    ("oxblood",          "Oxblood",          "near-black → deep oxblood red",        "#140004",  "#a01838" ),
    ("rosehip",          "Rosehip",          "dark rosehip → bright pink-red",       "#1c0208",  "#ff4070" ),
    ("strawberry",       "Strawberry",       "near-black → strawberry red",          "#200008",  "#ff3868" ),
    ("merlot",           "Merlot",           "dark wine → bright magenta-red",       "#1a0010",  "#a02058" ),
    ("cinnabar",         "Cinnabar",         "dark mineral → bright cinnabar red",   "#220400",  "#e84020" ),
    ("firebrick",        "Firebrick",        "near-black → firebrick red-orange",    "#1e0400",  "#c83020" ),

    // oranges
    ("tangerine",        "Tangerine",        "near-black → bright tangerine",        "#200c00",  "#ff8010" ),
    ("persimmon",        "Persimmon",        "dark → vivid persimmon orange",        "#240a00",  "#ff6020" ),
    ("apricot",          "Apricot",          "dark → soft apricot orange",           "#220e00",  "#ffa050" ),
    ("marigold",         "Marigold",         "dark → bright marigold orange",        "#241000",  "#ffb020" ),
    ("pumpkin",          "Pumpkin",          "near-black → pumpkin orange",          "#1e0a00",  "#ff7818" ),
    ("amberglow",        "Amber Glow",       "dark → glowing amber orange",          "#200c00",  "#ffa830" ),
    ("butterscotch",     "Butterscotch",     "dark → rich butterscotch",             "#221000",  "#ffc060" ),
    ("cantaloupe",       "Cantaloupe",       "dark → soft cantaloupe orange",        "#240e00",  "#ffb870" ),
    ("carrot",           "Carrot",           "near-black → carrot orange",           "#1e0800",  "#ff8020" ),
    ("clementine",       "Clementine",       "dark → bright clementine orange",     "#220a00",  "#ff7028" ),
    ("sienna",           "Sienna",           "dark earth → warm sienna",             "#1c0a04",  "#d87838" ),
    ("ochre",            "Ochre",            "dark → earthy ochre gold",             "#1e1200",  "#d0a020" ),
    ("copperflame",      "Copper Flame",     "near-black → bright copper-orange",    "#1a0800",  "#e87030" ),
    ("burntorange",      "Burnt Orange",     "dark → burnt orange",                  "#1c0800",  "#d86018" ),
    ("saffronglow",      "Saffron Glow",     "dark → glowing saffron orange",        "#200e00",  "#ffc020" ),
    ("honeyamber",       "Honey Amber",      "dark → rich honey amber",              "#221000",  "#ffb840" ),

    // yellows
    ("lemon",            "Lemon",            "dark → bright lemon yellow",           "#1e1c00",  "#fff830" ),
    ("canary",           "Canary",           "near-black → canary yellow",           "#201e00",  "#ffff40" ),
    ("banana",           "Banana",           "dark → soft banana yellow",            "#1e1a00",  "#ffe860" ),
    ("mustard",          "Mustard",          "dark → deep mustard yellow",           "#1c1600",  "#d8b020" ),
    ("goldenrod",        "Goldenrod",        "dark → bright goldenrod",              "#1e1600",  "#e8b820" ),
    ("daffodil",         "Daffodil",         "dark → bright daffodil yellow",        "#201e00",  "#fff060" ),
    ("buttercup",        "Buttercup",        "dark → warm buttercup yellow",         "#1e1a00",  "#ffe830" ),
    ("sunflower",        "Sunflower",        "dark → bold sunflower yellow",         "#201c00",  "#ffd820" ),
    ("chartreuseyellow", "Chartreuse Yellow","dark → sharp chartreuse-yellow",       "#181e00",  "#e0ff30" ),
    ("strawyellow",      "Straw Yellow",     "dark → soft straw yellow",             "#1c1c00",  "#f0e080" ),
    ("flaxen",           "Flaxen",           "dark → warm flaxen gold",              "#1a1800",  "#e8d060" ),
    ("duckling",         "Duckling",         "dark → soft duckling yellow",          "#1c1e00",  "#fff880" ),
    ("pineapple",        "Pineapple",        "dark → bright pineapple yellow",       "#1e1c00",  "#ffe030" ),
    ("custard",          "Custard",          "dark → creamy custard yellow",         "#1e1a00",  "#fff0a0" ),
    ("brimstone",        "Brimstone",        "dark → sharp brimstone yellow",        "#1c1c00",  "#e8ff40" ),
    ("topaz",            "Topaz",            "dark → warm topaz gold",               "#1e1600",  "#ffcc30" ),

    // greens / chartreuse / lime
    ("limezest",         "Lime Zest",        "dark → sharp lime green",              "#0c1a00",  "#b0ff20" ),
    ("chartreuseglow",   "Chartreuse Glow",  "dark → glowing chartreuse",            "#0e1c00",  "#c0ff10" ),
    ("springgreen",      "Spring Green",     "dark → bright spring green",           "#001c08",  "#40ff80" ),
    ("forestglow",       "Forest Glow",      "dark forest → bright green",           "#001608",  "#30d060" ),
    ("mossgreen",        "Moss Green",       "dark moss → soft green",               "#081406",  "#90c040" ),
    ("pistachio",        "Pistachio",        "dark → pale pistachio green",          "#0a1608",  "#b0e080" ),
    ("shamrock",         "Shamrock",         "near-black → shamrock green",          "#001208",  "#20d060" ),
    ("malachite",        "Malachite",        "dark → bright malachite green",        "#001008",  "#10e070" ),
    ("fernleaf",         "Fernleaf",         "dark fern → soft green",               "#0a1404",  "#a8d040" ),
    ("julep",            "Julep",            "dark → minty julep green",             "#001408",  "#90ffc0" ),
    ("avocado",          "Avocado",          "dark → warm avocado green",            "#0e1400",  "#b0d030" ),
    ("pear",             "Pear",             "dark → crisp pear green",              "#101800",  "#d0ff40" ),
    ("kiwi",             "Kiwi",             "dark → bright kiwi green",             "#0c1600",  "#a0e020" ),
    ("grasshopper",      "Grasshopper",      "dark → bright grass green",            "#0a1800",  "#90ff30" ),
    ("basil",            "Basil",            "dark → herby basil green",             "#081204",  "#70c040" ),
    ("clover",           "Clover",           "dark → lucky clover green",            "#0a1408",  "#60e070" ),
    ("verdant",          "Verdant",          "dark → rich verdant green",            "#001406",  "#40e070" ),

    // teals / cyan
    ("tealglow",         "Teal Glow",        "dark → glowing teal",                  "#001414",  "#20e0d0" ),
    ("turquoiseflame",   "Turquoise Flame",  "dark → bright turquoise",              "#001616",  "#10f0e0" ),
    ("aquamarine",       "Aquamarine",       "dark → bright aquamarine",             "#001414",  "#40ffd0" ),
    ("lagoonglow",       "Lagoon Glow",      "dark → glowing lagoon teal",           "#001210",  "#20d0b0" ),
    ("peacockteal",      "Peacock Teal",     "dark → rich peacock teal",             "#000e10",  "#10b0c0" ),
    ("tidepool",         "Tidepool",         "dark → bright tidepool cyan",          "#001416",  "#30e0f0" ),
    ("mintwave",         "Mint Wave",        "dark → soft mint-cyan",                "#001210",  "#60ffd8" ),
    ("seaglass",         "Seaglass",         "dark → pale seaglass teal",            "#001210",  "#80f0d0" ),
    ("oceanspray",       "Ocean Spray",      "dark → bright ocean cyan",             "#000e14",  "#40d0f0" ),
    ("cyanburst",        "Cyan Burst",       "dark → electric cyan",                 "#001818",  "#20fff0" ),
    ("tealfire",         "Teal Fire",        "dark → vivid teal",                    "#001010",  "#10e0c8" ),
    ("lapisteal",        "Lapis Teal",       "dark → deep lapis-teal",               "#000c12",  "#1090d0" ),
    ("capriblue",        "Capri Blue",       "dark → bright capri blue-cyan",        "#000e14",  "#30c0f0" ),
    ("mermaidglow",      "Mermaid Glow",     "dark → shimmering teal",               "#000e10",  "#20e8c8" ),
    ("kelpgreen",        "Kelp Green",       "dark → murky-bright kelp green",       "#0a1210",  "#40d0a0" ),
    ("tropicalteal",     "Tropical Teal",    "dark → bright tropical teal",          "#000f12",  "#30f0d0" ),
    ("riverstone",       "Riverstone",       "dark → cool river blue-teal",          "#000c0e",  "#50d0e0" ),

    // blues / azure
    ("azureflame",       "Azure Flame",      "near-black → bright azure",            "#00081e",  "#2080ff" ),
    ("skyburst",         "Sky Burst",        "dark → bright sky blue",               "#000a20",  "#40a0ff" ),
    ("cerulean",         "Cerulean",         "dark → vivid cerulean blue",           "#000818",  "#2090e0" ),
    ("electricblue",     "Electric Blue",    "near-black → electric blue",           "#000420",  "#2040ff" ),
    ("denimglow",        "Denim Glow",       "dark denim → soft blue",               "#00081a",  "#4070c0" ),
    ("sapphireblaze",    "Sapphire Blaze",   "dark → blazing sapphire",              "#000420",  "#2050e0" ),
    ("navyember",        "Navy Ember",       "deep navy → bright blue",              "#00041c",  "#1858d0" ),
    ("bluebell",         "Bluebell",         "dark → soft bluebell blue",            "#000818",  "#6090ff" ),
    ("oceanic",          "Oceanic",          "dark → rich ocean blue",               "#00061a",  "#2070d0" ),
    ("horizonblue",      "Horizon Blue",     "dark → bright horizon blue",           "#000a1c",  "#50a0ff" ),
    ("stormblue",        "Storm Blue",       "dark stormy → bright blue",            "#00061c",  "#3060c0" ),
    ("cobaltflame",      "Cobalt Flame",     "near-black → cobalt blue",             "#000420",  "#2050f0" ),
    ("midnightblue",     "Midnight Blue",    "near-black → bright midnight blue",    "#000420",  "#4060e0" ),
    ("frostblue",        "Frost Blue",       "dark → pale frost blue",               "#000818",  "#80c0ff" ),
    ("glacierblue",      "Glacier Blue",     "dark → bright glacier blue",           "#000a1e",  "#60c8ff" ),
    ("duskblue",         "Dusk Blue",        "dark dusk → muted bright blue",        "#00061a",  "#3050a0" ),
    ("zaffre",           "Zaffre",           "near-black → deep zaffre blue",        "#000420",  "#1040d0" ),

    // violets / indigo
    ("indigoflame",      "Indigo Flame",     "near-black → bright indigo",           "#08001c",  "#6020ff" ),
    ("violetblaze",      "Violet Blaze",     "dark → blazing violet",                "#0a0020",  "#8020ff" ),
    ("ultraviolet",      "Ultraviolet",      "near-black → ultraviolet",             "#06001c",  "#7000ff" ),
    ("irisglow",         "Iris Glow",        "dark → glowing iris purple",           "#08001a",  "#9040ff" ),
    ("amethystfire",     "Amethyst Fire",    "dark → bright amethyst",               "#0a0018",  "#a020ff" ),
    ("lavenderfire",     "Lavender Fire",    "dark → bright lavender-violet",        "#08001c",  "#b060ff" ),
    ("periwinklepop",    "Periwinkle Pop",   "dark → bright periwinkle",             "#06001c",  "#8080ff" ),
    ("mauveflame",       "Mauve Flame",      "dark → dusty bright mauve",            "#0a0018",  "#a050c0" ),
    ("wisteria",         "Wisteria",         "dark → soft wisteria purple",          "#08001a",  "#b080ff" ),
    ("grapefire",        "Grape Fire",       "dark → bright grape purple",           "#0a0016",  "#9020c0" ),
    ("plumblaze",        "Plum Blaze",       "dark plum → bright magenta-plum",      "#08000e",  "#a03080" ),
    ("eggplant",         "Eggplant",         "near-black → rich eggplant purple",    "#06000c",  "#800060" ),
    ("violetstorm",      "Violet Storm",     "dark → stormy bright violet",          "#08001a",  "#7040ff" ),
    ("inkviolet",        "Ink Violet",       "near-black → deep ink violet",         "#06000e",  "#6020a0" ),
    ("purplehaze",       "Purple Haze",      "dark → hazy bright purple",            "#08001c",  "#9060ff" ),
    ("royalviolet",      "Royal Violet",     "dark → rich royal violet",             "#0a0018",  "#7020e0" ),
    ("darkmagic",        "Dark Magic",       "near-black → deep magic purple",       "#06000e",  "#6010a0" ),

    // magenta / pink
    ("magentablaze",     "Magenta Blaze",    "dark → blazing magenta",               "#1a0016",  "#ff20c0" ),
    ("fuchsiaflame",     "Fuchsia Flame",    "dark → bright fuchsia",                "#180014",  "#ff30d0" ),
    ("hotpinkglow",      "Hot Pink Glow",    "dark → glowing hot pink",              "#1c000e",  "#ff2090" ),
    ("rouge",            "Rouge",            "dark → bright rouge pink-red",         "#1a000c",  "#ff3070" ),
    ("carminepink",      "Carmine Pink",     "dark → vivid carmine pink",            "#1a0008",  "#ff4080" ),
    ("punchpink",        "Punch Pink",       "dark → punchy bright pink",            "#1c000e",  "#ff1888" ),
    ("watermelon",       "Watermelon",       "dark → juicy watermelon pink-red",     "#1a0008",  "#ff5060" ),
    ("peonypink",        "Peony Pink",       "dark → soft peony pink",               "#180010",  "#ff60a0" ),
    ("azaleaglow",       "Azalea Glow",      "dark → glowing azalea pink",           "#180012",  "#ff70b0" ),
    ("raspberryflame",   "Raspberry Flame",  "dark → bright raspberry",              "#1a0006",  "#ff2868" ),
    ("lipstick",         "Lipstick",         "near-black → bold lipstick red-pink",  "#1c0008",  "#ff1858" ),
    ("berryburst",       "Berry Burst",      "dark berry → bright pink-red",         "#180008",  "#e83070" ),
    ("guavaglow",        "Guava Glow",       "dark → glowing guava pink",            "#1a0006",  "#ff4858" ),
    ("petuniapink",      "Petunia Pink",     "dark → rich petunia pink",             "#16000e",  "#e050b0" ),
    ("hibiscus",         "Hibiscus",         "dark → vivid hibiscus red-pink",       "#180008",  "#ff2050" ),
    ("camellia",         "Camellia",         "dark → bright camellia pink",          "#16000a",  "#ff4878" ),
    ("sweetheartpink",   "Sweetheart Pink",  "dark → soft sweetheart pink",          "#180010",  "#ff80b0" ),

    // browns / earth / neutrals
    ("mahogany",         "Mahogany",         "dark wood → warm mahogany red-brown",  "#180a04",  "#a04818" ),
    ("walnut",           "Walnut",           "dark → warm walnut brown",             "#140c06",  "#886040" ),
    ("chestnut",         "Chestnut",         "dark → rich chestnut brown-red",       "#180804",  "#a85028" ),
    ("umber",            "Umber",            "dark → warm umber brown",              "#140a04",  "#886030" ),
    ("cocoa",            "Cocoa",            "dark → rich cocoa brown",              "#160a06",  "#906038" ),
    ("espresso",         "Espresso",         "near-black → dark espresso brown",     "#120806",  "#6a4028" ),
    ("tobacco",          "Tobacco",          "dark → warm tobacco brown",            "#160c04",  "#a07830" ),
    ("saddlebrown",      "Saddle Brown",     "dark → rich saddle brown",             "#140804",  "#a05820" ),
    ("cedarwood",        "Cedarwood",        "dark → warm cedar brown",              "#160a04",  "#986040" ),
    ("clay",             "Clay",             "dark → warm terracotta clay",          "#1a0c06",  "#c07850" ),
    ("driftwood",        "Driftwood",        "dark → pale driftwood tan",            "#14100a",  "#b09070" ),
    ("sandstone",        "Sandstone",        "dark → warm sandstone tan",            "#16120a",  "#d8b888" ),
    ("taupe",            "Taupe",            "dark → soft taupe grey-brown",         "#14100c",  "#b09880" ),
    ("khaki",            "Khaki",            "dark → warm khaki tan",                "#161206",  "#c8b060" ),
    ("oatmeal",          "Oatmeal",          "dark → pale oatmeal tan",              "#16140e",  "#d8c8a0" ),
    ("almond",           "Almond",           "dark → soft almond tan",               "#181008",  "#d8b088" ),
    ("graphite",         "Graphite",         "near-black → neutral graphite grey",   "#101010",  "#909090" ),

    // grays / silvers
    ("silverstreak",     "Silver Streak",    "near-black → bright silver",           "#101012",  "#e0e0e8" ),
    ("platinum",         "Platinum",         "dark → cool platinum white",           "#0e0e10",  "#d8d8e0" ),
    ("pewter",           "Pewter",           "dark → muted pewter grey",             "#101012",  "#b8b8c0" ),
    ("gunmetal",         "Gunmetal",         "near-black → cool gunmetal grey",      "#0c0c10",  "#a0a8b0" ),
    ("slateglow",        "Slate Glow",       "dark → soft slate blue-grey",          "#0e1012",  "#c0c8d0" ),
    ("mercury",          "Mercury",          "dark → shiny mercury silver",          "#0e0e10",  "#d0d0d8" ),
    ("chrome",           "Chrome",           "near-black → bright chrome white",     "#101010",  "#e8e8f0" ),
    ("ash",              "Ash",              "dark → neutral ash grey",              "#101010",  "#c8c8c8" ),
    ("smoke",            "Smoke",            "near-black → soft smoke grey",         "#0e0e0e",  "#b8b8b8" ),
    ("fog",              "Fog",              "dark → pale foggy grey-blue",          "#101012",  "#d0d4d8" ),

    // jewel tones
    ("peridot",          "Peridot",          "dark → bright peridot green",          "#0e1400",  "#c0e030" ),
    ("tourmaline",       "Tourmaline",       "dark → bright tourmaline teal-green",  "#001210",  "#30e0a0" ),
    ("tanzanite",        "Tanzanite",        "dark → deep tanzanite blue-violet",    "#08001c",  "#5040ff" ),
    ("morganite",        "Morganite",        "dark → soft morganite pink",           "#1a0810",  "#ff90b0" ),
    ("aquagem",          "Aquagem",          "dark → bright aqua gem blue",          "#000e14",  "#30d0f0" ),
    ("citrinegem",       "Citrine Gem",      "dark → bright citrine yellow",         "#1a1400",  "#ffcc20" ),
    ("spinelred",        "Spinel Red",       "dark → vivid spinel red",              "#1a0004",  "#ff2040" ),
    ("zirconblue",       "Zircon Blue",      "dark → bright zircon blue",            "#000818",  "#40a0ff" ),
    ("opalwhite",        "Opal White",       "dark → soft opalescent white",         "#101014",  "#f0f0ff" ),
    ("onyxglow",         "Onyx Glow",        "near-black → neutral onyx grey",       "#0a0a0a",  "#808080" ),

    // cosmic / space
    ("starlightblue",    "Starlight Blue",   "near-black → pale starlight blue",     "#000420",  "#a0c0ff" ),
    ("cosmicpurple",     "Cosmic Purple",    "dark → bright cosmic purple",          "#0a0018",  "#c080ff" ),
    ("supernova",        "Supernova",        "dark → blinding supernova yellow",     "#1a0400",  "#ffe040" ),
    ("blackhole",        "Black Hole",       "true black → deep violet edge glow",   "#020204",  "#402060" ),
    ("pulsarpink",       "Pulsar Pink",      "dark → bright pulsar pink",            "#180010",  "#ff60d0" ),
    ("novawhite",        "Nova White",       "near-black → pure white flash",        "#101014",  "#ffffff" ),
    ("comettail",        "Comet Tail",       "dark → icy comet-tail blue",           "#000818",  "#a0e0ff" ),
    ("asteroidgray",     "Asteroid Gray",    "dark → dusty asteroid grey",           "#0c0c0e",  "#a0a0a8" ),
    ("meteorstreak",     "Meteor Streak",    "dark → bright meteor orange",          "#1a0800",  "#ffb060" ),
    ("stardust",         "Stardust",         "dark → shimmering pale blue-violet",   "#0a0a10",  "#d0d0ff" ),

    // food / dessert
    ("cottoncherry",     "Cotton Cherry",    "dark → bright cherry pink",            "#1a0006",  "#ff6080" ),
    ("blueberrypie",     "Blueberry Pie",    "dark → bright blueberry blue-violet",  "#04001a",  "#6060ff" ),
    ("limesorbet",       "Lime Sorbet",      "dark → bright lime sorbet green",      "#0a1400",  "#c0ff60" ),
    ("peachcobbler",     "Peach Cobbler",    "dark → soft peach orange",             "#1a0e00",  "#ffb080" ),
    ("grapesoda",        "Grape Soda",       "dark → bright grape purple",           "#0e0018",  "#a060ff" ),
    ("tangerinedream",   "Tangerine Dream",  "dark → bold tangerine orange",         "#1e0c00",  "#ff9020" ),
    ("mintchip",         "Mint Chip",        "dark → soft minty green",              "#001410",  "#80ffd0" ),
    ("rootbeer",         "Root Beer",        "dark → warm root beer brown",          "#140a04",  "#906040" ),
    ("bubblegumpop",     "Bubblegum Pop",    "dark → bright bubblegum pink",         "#1a0012",  "#ff80d0" ),
    ("cherrycola",       "Cherry Cola",      "dark → deep cherry-cola red",          "#180004",  "#c02040" ),

    // misc / unique
    ("emberash",         "Ember Ash",        "dark ash → glowing ember orange",      "#1a0800",  "#ff8040" ),
    ("frostfire",        "Frost Fire",       "dark → icy bright blue",               "#000a1a",  "#80e0ff" ),
    ("duskember",        "Dusk Ember",       "dark dusk → warm ember pink",          "#1a0410",  "#ff6088" ),
    ("mosslight",        "Moss Light",       "dark moss → bright yellow-green",      "#0a1204",  "#a0d040" ),
    ("tidalteal",        "Tidal Teal",       "dark → bright tidal teal",             "#000e10",  "#40e0d0" ),
    ("cinderglow",       "Cinder Glow",      "dark cinder → bright orange-red",      "#1a0400",  "#ff5020" ),
    ("duneglow",         "Dune Glow",        "dark → warm sandy dune glow",          "#1a1206",  "#ffcc80" ),
    ("glowworm",         "Glowworm",         "dark → glowing yellow-green",          "#0a1400",  "#d0ff40" ),
];



// ---------------------------------------------------------------------
// Help & color list (colorized)
// ---------------------------------------------------------------------

fn print_banner() {
    // A short fire-colored ASCII logo rendered with real ANSI RGB.
    let lines = [
        "  ██████╗ ██╗   ██╗██████╗  ██████╗  ██████╗██╗     ███████╗ █████╗ ██████╗ ",
        "  ██╔══██╗╚██╗ ██╔╝██╔══██╗██╔═══██╗██╔════╝██║     ██╔════╝██╔══██╗██╔══██╗",
        "  ██████╔╝ ╚████╔╝ ██████╔╝██║   ██║██║     ██║     █████╗  ███████║██████╔╝",
        "  ██╔═══╝   ╚██╔╝  ██╔══██╗██║   ██║██║     ██║     ██╔══╝  ██╔══██║██╔══██╗",
        "  ██║        ██║   ██║  ██║╚██████╔╝╚██████╗███████╗███████╗██║  ██║██║  ██║",
        "  ╚═╝        ╚═╝   ╚═╝  ╚═╝ ╚═════╝  ╚═════╝╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝",
    ];
    // Gradient from deep red at top to bright orange at bottom
    let colors: [(u8, u8, u8); 6] = [
        (180, 10, 0), (210, 30, 0), (230, 60, 0),
        (240, 100, 0), (250, 160, 10), (255, 200, 30),
    ];
    for (line, &(r, g, b)) in lines.iter().zip(colors.iter()) {
        println!("{ESC}[38;2;{r};{g};{b}m{line}{ESC}[0m");
    }
    println!();
}

fn print_help() {
    print_banner();
    println!("{ESC}[1;37mUSAGE{ESC}[0m");
    println!("    pyroclear [OPTIONS]\n");
    println!("{ESC}[1;37mOPTIONS{ESC}[0m");
    println!("    {ESC}[38;2;255;160;40m--color{ESC}[0m {ESC}[38;2;200;200;200m<name>{ESC}[0m          Named palette (see --list-colors)");
    println!("    {ESC}[38;2;255;160;40m--pick{ESC}[0m, {ESC}[38;2;255;160;40m-p{ESC}[0m              Interactive TUI color picker");
    println!("    {ESC}[38;2;255;160;40m--from{ESC}[0m {ESC}[38;2;200;200;200m<hex>{ESC}[0m {ESC}[38;2;255;160;40m--to{ESC}[0m {ESC}[38;2;200;200;200m<hex>{ESC}[0m  Custom gradient, e.g. --from \"#1a0000\" --to \"#ffcc00\"");
    println!("    {ESC}[38;2;255;160;40m--list-colors{ESC}[0m           List palettes with live color swatches");
    println!("    {ESC}[38;2;255;160;40m-h{ESC}[0m, {ESC}[38;2;255;160;40m--help{ESC}[0m             Show this help\n");
    println!("{ESC}[38;2;150;150;150mYour choice is saved to ~/.config/pyroclear/config.toml{ESC}[0m");
    println!("{ESC}[38;2;150;150;150mand reused automatically next time you run pyroclear with no flags.{ESC}[0m");
}

/// Render a gradient swatch of `width` cells between two colors.
fn swatch(from: (u8, u8, u8), to: (u8, u8, u8), width: usize) -> String {
    let mut s = String::new();
    for i in 0..width {
        let t = i as f32 / (width - 1) as f32;
        let r = (from.0 as f32 + t * (to.0 as f32 - from.0 as f32)).round() as u8;
        let g = (from.1 as f32 + t * (to.1 as f32 - from.1 as f32)).round() as u8;
        let b = (from.2 as f32 + t * (to.2 as f32 - from.2 as f32)).round() as u8;
        s.push_str(&format!("{ESC}[48;2;{r};{g};{b}m "));
    }
    s.push_str(&format!("{ESC}[0m"));
    s
}

fn palette_swatch(palette: &Palette, width: usize) -> String {
    let mut s = String::new();
    let step = 36.0 / (width - 1) as f32;
    for i in 0..width {
        let idx = (i as f32 * step).round().clamp(1.0, 36.0) as usize;
        let (r, g, b) = palette[idx];
        s.push_str(&format!("{ESC}[48;2;{r};{g};{b}m "));
    }
    s.push_str(&format!("{ESC}[0m"));
    s
}

fn print_color_list() {
    print_banner();
    println!("{ESC}[1;37mAvailable palettes:{ESC}[0m\n");

    for (id, display, desc, from_hex, to_hex) in NAMED_PALETTES {
        let sw = if *id == "fire" {
            let p = soften(&FIRE_PALETTE, SOFTEN_DESATURATE, SOFTEN_BRIGHTEN);
            palette_swatch(&p, 28)
        } else {
            let from = hex_to_rgb(from_hex).unwrap_or((0, 0, 0));
            let to = hex_to_rgb(to_hex).unwrap_or((255, 255, 255));
            // Build the full palette so the swatch matches what actually burns
            let p = soften(&generate_palette(from, to), SOFTEN_DESATURATE, SOFTEN_BRIGHTEN);
            palette_swatch(&p, 28)
        };
        println!(
            "  {ESC}[1;38;2;255;200;80m{id:<8}{ESC}[0m  {sw}  {ESC}[38;2;180;180;180m{desc}{ESC}[0m"
        );
        let _ = display; // used in picker, suppress unused warning
    }
    println!("\n{ESC}[38;2;150;150;150mCustom gradient: pyroclear --from \"#rrggbb\" --to \"#rrggbb\"{ESC}[0m");
    println!("{ESC}[38;2;150;150;150mInteractive:     pyroclear --pick{ESC}[0m");
}

// ---------------------------------------------------------------------
// Color math: hex <-> rgb <-> hsv, gradient generator
// ---------------------------------------------------------------------

fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some((r, g, b))
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;

    let h = if delta == 0.0 {
        0.0
    } else if max == rf {
        60.0 * (((gf - bf) / delta).rem_euclid(6.0))
    } else if max == gf {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };

    let s = if max == 0.0 { 0.0 } else { delta / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let hh = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (hh.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hh as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn lerp_hue(a: f32, b: f32, t: f32) -> f32 {
    let mut d = b - a;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    (a + d * t).rem_euclid(360.0)
}

/// Build a 37-step ramp between two colors, interpolated in HSV.
fn generate_palette(from: (u8, u8, u8), to: (u8, u8, u8)) -> Palette {
    let (mut h0, mut s0, v0) = rgb_to_hsv(from.0, from.1, from.2);
    let (h1, s1, v1) = rgb_to_hsv(to.0, to.1, to.2);
    if v0 < 0.15 {
        h0 = h1;
        s0 = s1 * 0.7;
    }
    let v0 = v0.max(0.08);

    let mut out: Palette = [(0, 0, 0); 37];
    for (i, slot) in out.iter_mut().enumerate() {
        let t = i as f32 / 36.0;
        let h = lerp_hue(h0, h1, t);
        let s = (s0 + t * (s1 - s0)).clamp(0.0, 1.0);
        let v = (v0 + t * (v1 - v0)).clamp(0.0, 1.0);
        *slot = hsv_to_rgb(h, s, v);
    }
    out
}

// Final look-adjustment constants. Applied as a pass over every palette
// right before it's used for a swatch or an actual burn, so what you see
// in --list-colors / --pick always matches what actually renders.
//
// SOFTEN_DESATURATE: multiplies saturation (< 1.0 = less saturated/more washed out)
// SOFTEN_BRIGHTEN:   pulls value toward 1.0 (white) by this fraction of the remaining headroom
const SOFTEN_DESATURATE: f32 = 0.62;
const SOFTEN_BRIGHTEN: f32 = 0.32;

/// Soften a palette: reduce saturation and lift brightness toward white.
/// Replaces the old `boost_saturation` pass — instead of punchier/more
/// saturated colors, this produces a brighter, gentler, less-saturated look.
fn soften(palette: &Palette, desaturate_factor: f32, brighten_factor: f32) -> Palette {
    let mut out = *palette;
    for slot in out.iter_mut() {
        let (h, s, v) = rgb_to_hsv(slot.0, slot.1, slot.2);
        let new_s = (s * desaturate_factor).clamp(0.0, 1.0);
        let new_v = (v + (1.0 - v) * brighten_factor).clamp(0.0, 1.0);
        *slot = hsv_to_rgb(h, new_s, new_v);
    }
    out
}

// ---------------------------------------------------------------------
// Palette selection: CLI args -> saved config -> default
// ---------------------------------------------------------------------

enum PaletteChoice {
    Named(String),
    Custom { from: (u8, u8, u8), to: (u8, u8, u8) },
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/pyroclear/config.toml"))
}

fn load_config() -> Option<PaletteChoice> {
    let path = config_path()?;
    let content = std::fs::read_to_string(path).ok()?;

    let mut palette = None;
    let mut from = None;
    let mut to = None;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('[') || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let val = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "palette" => palette = Some(val),
                "from" => from = Some(val),
                "to" => to = Some(val),
                _ => {}
            }
        }
    }

    if let (Some(f), Some(t)) = (from, to) {
        if let (Some(fc), Some(tc)) = (hex_to_rgb(&f), hex_to_rgb(&t)) {
            return Some(PaletteChoice::Custom { from: fc, to: tc });
        }
    }
    palette.map(PaletteChoice::Named)
}

fn save_config(choice: &PaletteChoice) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = match choice {
        PaletteChoice::Named(name) => format!("[color]\npalette = \"{name}\"\n"),
        PaletteChoice::Custom { from, to } => format!(
            "[color]\nfrom = \"#{:02x}{:02x}{:02x}\"\nto = \"#{:02x}{:02x}{:02x}\"\n",
            from.0, from.1, from.2, to.0, to.1, to.2
        ),
    };
    let _ = std::fs::write(path, content);
}

fn validate_named(name: &str) -> Result<(), String> {
    if NAMED_PALETTES.iter().any(|(id, _, _, _, _)| *id == name) {
        Ok(())
    } else {
        let valid: Vec<&str> = NAMED_PALETTES.iter().map(|(id, _, _, _, _)| *id).collect();
        Err(format!(
            "Unknown palette '{name}'. Valid names: {}\nTry --list-colors or --pick for an interactive selector.",
            valid.join(", ")
        ))
    }
}

/// Parses CLI flags. Returns Some(choice) if the user passed color flags.
fn parse_args() -> Option<PaletteChoice> {
    let args: Vec<String> = std::env::args().collect();
    let mut color = None;
    let mut from = None;
    let mut to = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--color" | "-c" => {
                i += 1;
                color = args.get(i).cloned();
            }
            "--from" => {
                i += 1;
                from = args.get(i).cloned();
            }
            "--to" => {
                i += 1;
                to = args.get(i).cloned();
            }
            "--list-colors" | "--list" => {
                print_color_list();
                std::process::exit(0);
            }
            "--pick" | "-p" => {
                let picked = interactive_pick();
                match picked {
                    Some(c) => {
                        save_config(&c);
                        return Some(c);
                    }
                    None => std::process::exit(0),
                }
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    if from.is_some() || to.is_some() {
        let (Some(f), Some(t)) = (from, to) else {
            eprintln!("{ESC}[1;31merror:{ESC}[0m --from and --to must be used together, e.g.\n  pyroclear --from \"#1a0000\" --to \"#ffcc00\"");
            std::process::exit(1);
        };
        let (Some(fc), Some(tc)) = (hex_to_rgb(&f), hex_to_rgb(&t)) else {
            eprintln!("{ESC}[1;31merror:{ESC}[0m Invalid hex color(s) — expected format #rrggbb");
            std::process::exit(1);
        };
        return Some(PaletteChoice::Custom { from: fc, to: tc });
    }

    if let Some(name) = color {
        if let Err(e) = validate_named(&name) {
            eprintln!("{ESC}[1;31merror:{ESC}[0m {e}");
            std::process::exit(1);
        }
        return Some(PaletteChoice::Named(name));
    }

    None
}

fn resolve_choice() -> PaletteChoice {
    if let Some(choice) = parse_args() {
        save_config(&choice);
        return choice;
    }
    load_config().unwrap_or(PaletteChoice::Named("fire".to_string()))
}

fn build_palette(choice: &PaletteChoice) -> Palette {
    let raw = match choice {
        PaletteChoice::Named(name) => match name.as_str() {
            "fire" => FIRE_PALETTE,
            other => {
                if let Some((_, _, _, from_hex, to_hex)) =
                    NAMED_PALETTES.iter().find(|(id, _, _, _, _)| *id == other)
                {
                    let from = hex_to_rgb(from_hex).unwrap_or((0, 0, 0));
                    let to = hex_to_rgb(to_hex).unwrap_or((255, 255, 255));
                    generate_palette(from, to)
                } else {
                    // Should have been caught by validate_named, but be safe
                    eprintln!("{ESC}[1;31merror:{ESC}[0m Unknown palette '{other}', using fire.");
                    FIRE_PALETTE
                }
            }
        },
        PaletteChoice::Custom { from, to } => generate_palette(*from, *to),
    };

    soften(&raw, SOFTEN_DESATURATE, SOFTEN_BRIGHTEN)
}

// ---------------------------------------------------------------------
// Interactive TUI color picker (curses-style, pure libc + ANSI)
// ---------------------------------------------------------------------

/// RAII guard: enters raw mode on creation, restores termios on drop.
/// Also switches to the alternate screen buffer.
struct TermRawGuard {
    orig: libc::termios,
}

impl TermRawGuard {
    fn enter() -> io::Result<Self> {
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut orig) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = orig;
        // Disable canonical mode, echo, signal chars
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
        // Read returns after 1 byte, no timeout
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) };

        // Enter alternate screen, hide cursor
        print!("{ESC}[?1049h{ESC}[?25l");
        io::stdout().flush().ok();

        Ok(Self { orig })
    }
}

impl Drop for TermRawGuard {
    fn drop(&mut self) {
        // Leave alternate screen, show cursor
        print!("{ESC}[?1049l{ESC}[?25h");
        io::stdout().flush().ok();
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.orig) };
    }
}

/// Read one "key event" from stdin in raw mode.
#[derive(Debug, PartialEq)]
enum Key {
    Up,
    Down,
    Enter,
    Char(char),
    Esc,
    Backspace,
    Other,
}

fn read_key() -> Key {
    let mut buf = [0u8; 4];
    let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut _, 4) };
    if n <= 0 {
        return Key::Other;
    }
    match &buf[..n as usize] {
        [0x1b, b'[', b'A', ..] => Key::Up,
        [0x1b, b'[', b'B', ..] => Key::Down,
        [0x1b, ..] if n == 1 => Key::Esc,
        [0x0d] | [0x0a] => Key::Enter,
        [0x7f] | [0x08] => Key::Backspace,
        [c] if *c >= 0x20 && *c < 0x7f => Key::Char(*c as char),
        _ => Key::Other,
    }
}

/// Prompt the user for a hex string in raw mode (with echo).
fn prompt_hex(label: &str, row: u16) -> Option<String> {
    let mut input = String::new();
    loop {
        // Clear row and redraw prompt
        print!("{ESC}[{row};1H{ESC}[2K{ESC}[38;2;255;200;80m{label}{ESC}[0m {input}_");
        io::stdout().flush().ok();
        match read_key() {
            Key::Enter => {
                let s = input.trim().to_string();
                if s.is_empty() {
                    return None;
                }
                if hex_to_rgb(&s).is_some() {
                    return Some(s);
                }
                // Invalid — flash an error hint
                print!("{ESC}[{row};1H{ESC}[2K{ESC}[38;2;255;60;60m  ✗ invalid hex (need #rrggbb){ESC}[0m");
                io::stdout().flush().ok();
                std::thread::sleep(Duration::from_millis(800));
                input.clear();
            }
            Key::Backspace => {
                input.pop();
            }
            Key::Char(c) => {
                if input.len() < 7 {
                    input.push(c);
                }
            }
            Key::Esc => return None,
            _ => {}
        }
    }
}

/// Draw the picker UI at the current terminal size.
fn draw_picker(selected: usize, (cols, rows): (usize, usize)) {
    let total = NAMED_PALETTES.len() + 1; // +1 for "Custom"
    let swatch_w = (cols.saturating_sub(30)).clamp(12, 36);

    // Title bar
    print!("{ESC}[H");
    let title = " pyroclear — color picker ";
    let hint = " [↑↓] navigate  [Enter] select  [q/Esc] cancel ";
    let gap = cols.saturating_sub(title.len() + hint.len());
    print!(
        "{ESC}[48;2;30;30;50m{ESC}[38;2;255;200;80m{title}{ESC}[38;2;140;140;160m{}{hint}{ESC}[0m",
        " ".repeat(gap)
    );

    // Palette rows
    let list_start = 2usize;
    let visible = rows.saturating_sub(list_start + 2); // leave bottom margin
    let offset = if selected >= visible { selected - visible + 1 } else { 0 };

    for slot in 0..visible {
        let idx = slot + offset;
        let row = (list_start + slot + 1) as u16;
        print!("{ESC}[{row};1H{ESC}[2K");

        if idx >= total {
            break;
        }

        let is_selected = idx == selected;
        let cursor = if is_selected { "▸" } else { " " };

        if is_selected {
            print!("{ESC}[48;2;20;25;40m");
        }

        let (name, desc, sw) = if idx < NAMED_PALETTES.len() {
            let (id, display, desc, from_hex, to_hex) = NAMED_PALETTES[idx];
            let p = if id == "fire" {
                soften(&FIRE_PALETTE, SOFTEN_DESATURATE, SOFTEN_BRIGHTEN)
            } else {
                let from = hex_to_rgb(from_hex).unwrap_or((0, 0, 0));
                let to = hex_to_rgb(to_hex).unwrap_or((255, 255, 255));
                soften(&generate_palette(from, to), SOFTEN_DESATURATE, SOFTEN_BRIGHTEN)
            };
            (display, desc, palette_swatch(&p, swatch_w))
        } else {
            // "Custom" entry
            let sw = swatch((60, 0, 60), (255, 200, 100), swatch_w);
            ("Custom", "enter your own #rrggbb gradient", sw)
        };

        if is_selected {
            print!("{ESC}[1;38;2;255;220;80m");
        } else {
            print!("{ESC}[38;2;200;200;210m");
        }

        print!(
    "  {cursor} {name:<18}    {sw}  {ESC}[38;2;140;140;160m{desc}{ESC}[0m"
    );

        if is_selected {
            // Fill rest of line with selection background
            print!("{ESC}[48;2;20;25;40m{}{ESC}[0m", " ".repeat(4));
        }
    }

    // Bottom separator
    let sep_row = (list_start + visible + 1) as u16;
    print!("{ESC}[{sep_row};1H{ESC}[2K{ESC}[38;2;60;60;80m{}{ESC}[0m", "─".repeat(cols));
    io::stdout().flush().ok();
}

/// Run the interactive palette picker. Returns the chosen PaletteChoice
/// or None if the user cancelled.
fn interactive_pick() -> Option<PaletteChoice> {
    let _guard = TermRawGuard::enter().ok()?;

    let total = NAMED_PALETTES.len() + 1; // +1 for custom
    let mut selected = 0usize;

    loop {
        let size = terminal_size();
        draw_picker(selected, size);

        match read_key() {
            Key::Up => {
                if selected > 0 {
                    selected -= 1;
                }
            }
            Key::Down => {
                if selected + 1 < total {
                    selected += 1;
                }
            }
            Key::Enter => {
                if selected < NAMED_PALETTES.len() {
                    let (id, _, _, _, _) = NAMED_PALETTES[selected];
                    return Some(PaletteChoice::Named(id.to_string()));
                } else {
                    // Custom hex input — clear area and prompt
                    let (_, rows) = terminal_size();
                    let base = rows as u16 - 4;
                    print!("{ESC}[{base};1H{ESC}[J");
                    print!("{ESC}[{base};1H{ESC}[38;2;180;180;200m  Enter hex colors (e.g. #ff0000){ESC}[0m\n");
                    io::stdout().flush().ok();

                    let from_str = prompt_hex("  From:", base + 2)?;
                    let to_str = prompt_hex("  To:  ", base + 3)?;

                    let from = hex_to_rgb(&from_str)?;
                    let to = hex_to_rgb(&to_str)?;
                    return Some(PaletteChoice::Custom { from, to });
                }
            }
            Key::Esc | Key::Char('q') => return None,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------
// PRNG
// ---------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos() as u64
            | 1;
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        let span = (hi - lo + 1) as u64;
        lo + (self.next_u64() % span) as i32
    }
}

// ---------------------------------------------------------------------
// Terminal + rendering
// ---------------------------------------------------------------------

fn terminal_size() -> (usize, usize) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            return (ws.ws_col as usize, ws.ws_row as usize);
        }
    }
    (80, 24)
}

#[derive(PartialEq, Clone, Copy)]
enum CellColor {
    Default,
    Rgb(u8, u8, u8),
}

fn render(buf: &mut String, grid: &[u8], cols: usize, rows: usize, palette: &Palette, label: &str) {
    buf.clear();
    buf.push_str(ESC);
    buf.push_str("[H");

    let mut last: Option<CellColor> = None;
    for y in 0..rows {
        for x in 0..cols {
            let heat = grid[y * cols + x];
            let color = if heat == 0 {
                CellColor::Default
            } else {
                let (r, g, b) = palette[heat as usize];
                CellColor::Rgb(r, g, b)
            };

            if last != Some(color) {
                match color {
                    CellColor::Default => buf.push_str(&format!("{ESC}[49m")),
                    CellColor::Rgb(r, g, b) => {
                        buf.push_str(&format!("{ESC}[48;2;{r};{g};{b}m"))
                    }
                }
                last = Some(color);
            }
            buf.push(' ');
        }
        buf.push('\n');
    }

    // Palette label: bottom-right corner, faint
    if !label.is_empty() && cols > label.len() + 2 {
        let col = cols - label.len() - 1;
        buf.push_str(&format!(
            "{ESC}[{rows};{col}H{ESC}[0m{ESC}[38;2;80;80;80m {label} {ESC}[0m"
        ));
    }
}

fn resize_grid(cols: usize, rows: usize) -> Vec<u8> {
    let mut grid = vec![0u8; cols * rows];
    for x in 0..cols {
        grid[(rows - 1) * cols + x] = MAX_HEAT;
    }
    grid
}

fn burn(palette: &Palette, label: &str, interrupted: Arc<AtomicBool>) {
    let (mut cols, mut rows) = terminal_size();
    let mut grid = resize_grid(cols, rows);
    let mut rng = Rng::new();

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = write!(out, "{ESC}[?25l{ESC}[2J"); // hide cursor, clear screen

    let start = Instant::now();
    let source_cool_at = MAX_DURATION.mul_f32(SOURCE_COOL_START);
    let mut frame = String::with_capacity(cols * rows * 8);

    loop {
        if interrupted.load(Ordering::Relaxed) {
            break;
        }
        let elapsed = start.elapsed();
        if elapsed > MAX_DURATION {
            break;
        }

        // live resize
        let (new_cols, new_rows) = terminal_size();
        if new_cols != cols || new_rows != rows {
            cols = new_cols;
            rows = new_rows;
            grid = resize_grid(cols, rows);
            frame.reserve(cols * rows * 8);
        }

        for _ in 0..STEPS_PER_FRAME {
            for x in 0..cols {
                for y in 1..rows {
                    let below = grid[y * cols + x];
                    let decay = rng.range(0, 3);
                    let drift = rng.range(-1, 1);
                    let nx = (x as i32 + drift).clamp(0, cols as i32 - 1) as usize;
                    let new_val = (below as i32 - decay).max(0) as u8;
                    grid[(y - 1) * cols + nx] = new_val;
                }
            }

            if elapsed > source_cool_at {
                for x in 0..cols {
                    let idx = (rows - 1) * cols + x;
                    let dec = rng.range(2, 6);
                    grid[idx] = (grid[idx] as i32 - dec).max(0) as u8;
                }
            }
        }

        render(&mut frame, &grid, cols, rows, palette, label);
        let _ = out.write_all(frame.as_bytes());
        let _ = out.flush();

        if elapsed > source_cool_at {
            let peak = grid.iter().copied().max().unwrap_or(0);
            if peak < DIE_OUT_THRESHOLD {
                break;
            }
        }

        std::thread::sleep(FRAME_DELAY);
    }

    let _ = write!(out, "{ESC}[?25h"); // always restore cursor
}

// ---------------------------------------------------------------------
// SIGINT handler
// ---------------------------------------------------------------------

/// Shared flag set by the C-level signal handler; polled by a watcher
/// thread that bridges it into an Arc<AtomicBool> the burn loop checks.
static SIGINT_FIRED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigint_handler(_: libc::c_int) {
    SIGINT_FIRED.store(true, Ordering::Relaxed);
}

/// Install SIGINT → set flag. Returns an Arc the burn loop polls each frame.
fn install_sigint() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let flag2 = Arc::clone(&flag);

    unsafe {
        libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t);
    }

    std::thread::spawn(move || loop {
        if SIGINT_FIRED.load(Ordering::Relaxed) {
            flag2.store(true, Ordering::Relaxed);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    });

    flag
}

// ---------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------

fn main() {
    let choice = resolve_choice();
    let palette = build_palette(&choice);

    let interrupted = install_sigint();

    burn(&palette, "", interrupted);

    // Final clear — always runs, even after SIGINT (cursor was restored in burn)
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = write!(out, "{ESC}[H{ESC}[2J");
    let _ = out.flush();
}