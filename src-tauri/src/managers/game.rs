use serde::{Deserialize, Serialize};
use crate::diag_log;
use std::fs;
use std::path::PathBuf;
use reqwest::Client;
use std::os::windows::process::CommandExt;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GameMetadata {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metacritic: Option<MetacriticInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_new: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nsfw: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MetacriticInfo {
    pub score: i32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SteamToolsGame {
    #[serde(default)]
    pub appid: serde_json::Value,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub header_image: Option<String>,
    #[serde(default)]
    pub updated_date: Option<String>,
    #[serde(default)]
    pub added_date: Option<String>,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub drm: bool,
    #[serde(rename = "type", default)]
    pub item_type: String,
}

pub static FALLBACK_GAME_IDS: &[&str] = &[
    "500",
    "550",
    "2280",
    "2310",
    "3590",
    "4000",
    "8080",
    "8190",
    "8980",
    "10090",
    "10150",
    "10180",
    "11450",
    "12110",
    "12120",
    "12140",
    "12150",
    "12200",
    "12210",
    "13540",
    "17390",
    "19680",
    "20510",
    "20900",
    "20920",
    "21690",
    "22300",
    "22320",
    "22370",
    "22380",
    "32450",
    "33230",
    "39140",
    "40990",
    "42680",
    "42700",
    "43110",
    "47890",
    "48190",
    "49520",
    "50300",
    "57400",
    "71340",
    "72850",
    "105600",
    "107410",
    "108600",
    "108710",
    "113200",
    "115320",
    "200210",
    "200260",
    "200940",
    "201810",
    "202530",
    "202970",
    "203160",
    "203650",
    "203770",
    "204100",
    "205100",
    "208650",
    "209000",
    "209100",
    "209120",
    "209160",
    "210550",
    "212480",
    "213610",
    "213670",
    "214490",
    "218620",
    "220240",
    "221040",
    "221100",
    "221910",
    "222480",
    "223710",
    "223850",
    "225540",
    "227100",
    "227300",
    "227940",
    "232010",
    "234140",
    "235460",
    "236850",
    "237110",
    "238320",
    "239030",
    "239140",
    "240720",
    "241540",
    "241930",
    "242050",
    "242550",
    "242700",
    "242760",
    "242960",
    "243800",
    "245490",
    "246620",
    "247910",
    "249130",
    "250900",
    "251570",
    "252490",
    "254700",
    "255710",
    "257850",
    "260210",
    "264710",
    "267490",
    "267530",
    "267550",
    "268050",
    "268910",
    "270880",
    "271590",
    "272510",
    "275850",
    "281990",
    "284160",
    "285900",
    "285920",
    "286690",
    "287290",
    "287390",
    "287700",
    "288880",
    "292000",
    "292030",
    "292120",
    "294100",
    "298110",
    "299970",
    "302510",
    "304240",
    "304390",
    "307780",
    "311210",
    "311340",
    "312520",
    "313120",
    "313690",
    "319510",
    "321360",
    "322170",
    "322330",
    "323850",
    "327030",
    "329050",
    "332800",
    "332950",
    "335300",
    "336140",
    "339340",
    "345330",
    "352400",
    "354140",
    "354380",
    "360430",
    "362890",
    "365590",
    "367520",
    "368500",
    "374320",
    "377160",
    "377840",
    "379720",
    "381210",
    "383980",
    "388090",
    "389730",
    "391220",
    "391540",
    "393080",
    "394360",
    "394690",
    "395200",
    "397540",
    "406350",
    "412020",
    "413150",
    "414340",
    "414700",
    "418370",
    "424370",
    "424840",
    "431960",
    "433340",
    "434650",
    "438740",
    "440900",
    "447040",
    "447270",
    "448280",
    "454650",
    "458430",
    "459040",
    "460930",
    "474960",
    "477160",
    "485430",
    "488790",
    "489360",
    "489830",
    "490110",
    "492720",
    "495420",
    "498240",
    "503560",
    "504230",
    "506610",
    "513710",
    "517630",
    "518790",
    "521890",
    "526870",
    "534380",
    "536270",
    "546560",
    "552520",
    "553850",
    "556640",
    "560170",
    "570940",
    "573090",
    "577940",
    "579180",
    "582010",
    "584400",
    "586140",
    "588650",
    "594650",
    "601050",
    "601150",
    "601430",
    "604540",
    "612880",
    "617160",
    "617290",
    "619540",
    "621060",
    "622650",
    "627270",
    "629520",
    "629730",
    "631510",
    "637100",
    "637650",
    "648800",
    "655500",
    "667970",
    "668580",
    "674940",
    "675260",
    "678950",
    "678960",
    "699130",
    "728880",
    "731490",
    "732690",
    "739630",
    "747660",
    "750920",
    "751630",
    "751780",
    "752480",
    "752590",
    "752900",
    "753640",
    "755500",
    "759740",
    "774171",
    "782330",
    "784150",
    "789050",
    "789910",
    "800270",
    "801800",
    "805550",
    "812140",
    "814380",
    "815370",
    "816020",
    "829110",
    "834910",
    "835570",
    "848450",
    "850170",
    "850190",
    "851850",
    "855740",
    "859570",
    "860510",
    "861650",
    "863550",
    "865360",
    "870780",
    "872670",
    "883710",
    "897030",
    "899770",
    "905450",
    "916840",
    "920210",
    "924970",
    "928960",
    "934700",
    "939960",
    "945360",
    "946010",
    "949230",
    "952060",
    "960090",
    "960420",
    "960910",
    "960990",
    "962130",
    "962400",
    "967050",
    "976310",
    "976730",
    "977950",
    "978300",
    "990080",
    "991560",
    "993090",
    "997070",
    "999220",
    "999730",
    "1009290",
    "1012880",
    "1020790",
    "1027990",
    "1030300",
    "1030830",
    "1030840",
    "1032430",
    "1058450",
    "1063730",
    "1066890",
    "1078760",
    "1086940",
    "1090390",
    "1091500",
    "1095480",
    "1095650",
    "1097840",
    "1100910",
    "1110910",
    "1114150",
    "1118200",
    "1118520",
    "1138850",
    "1142500",
    "1142710",
    "1144200",
    "1145350",
    "1151340",
    "1151640",
    "1153410",
    "1154030",
    "1158310",
    "1159690",
    "1167630",
    "1169040",
    "1170950",
    "1172380",
    "1172620",
    "1172710",
    "1173820",
    "1174180",
    "1179210",
    "1180660",
    "1182900",
    "1184050",
    "1190000",
    "1194630",
    "1196590",
    "1201700",
    "1203220",
    "1203620",
    "1206560",
    "1211020",
    "1214650",
    "1217060",
    "1222140",
    "1222670",
    "1222680",
    "1222700",
    "1229490",
    "1230170",
    "1230530",
    "1230800",
    "1237320",
    "1237950",
    "1237970",
    "1238000",
    "1238060",
    "1238080",
    "1238810",
    "1238820",
    "1238840",
    "1238860",
    "1240440",
    "1245620",
    "1249970",
    "1250410",
    "1256670",
    "1259420",
    "1260320",
    "1262240",
    "1262540",
    "1262560",
    "1262580",
    "1262600",
    "1272080",
    "1273540",
    "1274570",
    "1275890",
    "1281590",
    "1281930",
    "1282100",
    "1284190",
    "1285190",
    "1290000",
    "1293830",
    "1304930",
    "1313140",
    "1313860",
    "1321680",
    "1326470",
    "1332010",
    "1335830",
    "1340990",
    "1341290",
    "1342490",
    "1346010",
    "1353230",
    "1356240",
    "1359090",
    "1366800",
    "1372110",
    "1373090",
    "1373220",
    "1374490",
    "1378990",
    "1392860",
    "1401730",
    "1422450",
    "1426210",
    "1430190",
    "1446780",
    "1452250",
    "1465360",
    "1466060",
    "1466640",
    "1477940",
    "1501750",
    "1506830",
    "1511460",
    "1511630",
    "1527950",
    "1546970",
    "1547000",
    "1551360",
    "1562430",
    "1566160",
    "1566880",
    "1567020",
    "1575940",
    "1576420",
    "1577120",
    "1578390",
    "1579340",
    "1583230",
    "1583720",
    "1590910",
    "1592190",
    "1593500",
    "1594320",
    "1601570",
    "1601580",
    "1604030",
    "1604270",
    "1607680",
    "1620730",
    "1623730",
    "1625790",
    "1627720",
    "1640820",
    "1641960",
    "1643320",
    "1649240",
    "1657630",
    "1659040",
    "1659420",
    "1660080",
    "1671210",
    "1672970",
    "1676840",
    "1677280",
    "1680880",
    "1686940",
    "1687950",
    "1689620",
    "1690940",
    "1703340",
    "1715590",
    "1716740",
    "1721060",
    "1721110",
    "1721470",
    "1734680",
    "1745310",
    "1750030",
    "1763050",
    "1766060",
    "1770190",
    "1771300",
    "1774580",
    "1778820",
    "1787090",
    "1787800",
    "1790600",
    "1794960",
    "1796470",
    "1804170",
    "1805480",
    "1808500",
    "1811260",
    "1812410",
    "1817070",
    "1817190",
    "1817230",
    "1836730",
    "1837480",
    "1842730",
    "1849900",
    "1850570",
    "1858630",
    "1864880",
    "1866130",
    "1868140",
    "1869500",
    "1872860",
    "1874880",
    "1877020",
    "1888160",
    "1888930",
    "1902960",
    "1903340",
    "1911610",
    "1913370",
    "1922560",
    "1925560",
    "1928870",
    "1929610",
    "1931180",
    "1934570",
    "1934680",
    "1940340",
    "1941540",
    "1943950",
    "1946550",
    "1957780",
    "1962700",
    "1963610",
    "1966720",
    "1969370",
    "1973530",
    "1974230",
    "1977170",
    "1984270",
    "1985820",
    "1987400",
    "1988550",
    "1990110",
    "2000950",
    "2001120",
    "2017610",
    "2019760",
    "2022670",
    "2027330",
    "2050650",
    "2054970",
    "2055290",
    "2058030",
    "2058190",
    "2062430",
    "2069250",
    "2076010",
    "2086680",
    "2088760",
    "2092840",
    "2093010",
    "2101960",
    "2104890",
    "2110820",
    "2120900",
    "2124490",
    "2128020",
    "2131630",
    "2131640",
    "2131650",
    "2135150",
    "2138710",
    "2141730",
    "2144740",
    "2157710",
    "2169200",
    "2172010",
    "2172260",
    "2183900",
    "2186990",
    "2195250",
    "2205460",
    "2208920",
    "2214220",
    "2215200",
    "2215390",
    "2215430",
    "2221390",
    "2221490",
    "2221920",
    "2223840",
    "2231380",
    "2233120",
    "2235200",
    "2239550",
    "2239710",
    "2246340",
    "2246670",
    "2249160",
    "2259310",
    "2275020",
    "2277560",
    "2282350",
    "2287520",
    "2291340",
    "2300320",
    "2322010",
    "2324290",
    "2325290",
    "2338770",
    "2340870",
    "2344520",
    "2348730",
    "2350790",
    "2351330",
    "2352620",
    "2353060",
    "2358260",
    "2358720",
    "2361770",
    "2362050",
    "2369390",
    "2373990",
    "2374190",
    "2379780",
    "2384580",
    "2397300",
    "2399420",
    "2399830",
    "2400430",
    "2404880",
    "2405060",
    "2406770",
    "2407270",
    "2416450",
    "2417610",
    "2420110",
    "2425290",
    "2427410",
    "2427420",
    "2427430",
    "2427520",
    "2428810",
    "2440510",
    "2445690",
    "2453160",
    "2456410",
    "2456740",
    "2457220",
    "2458830",
    "2461850",
    "2473480",
    "2478970",
    "2479290",
    "2483190",
    "2484110",
    "2486820",
    "2487150",
    "2488370",
    "2492040",
    "2502780",
    "2503770",
    "2513280",
    "2515050",
    "2522520",
    "2523720",
    "2524700",
    "2526310",
    "2527390",
    "2527500",
    "2529170",
    "2531310",
    "2537010",
    "2537590",
    "2538880",
    "2542120",
    "2543180",
    "2543830",
    "2545710",
    "2559270",
    "2564520",
    "2567870",
    "2569170",
    "2582300",
    "2584990",
    "2589820",
    "2592160",
    "2592220",
    "2593900",
    "2609610",
    "2611170",
    "2613950",
    "2615540",
    "2616140",
    "2622380",
    "2623190",
    "2624870",
    "2625420",
    "2627260",
    "2634950",
    "2637170",
    "2638370",
    "2638560",
    "2646460",
    "2651280",
    "2661300",
    "2668510",
    "2669320",
    "2669380",
    "2670630",
    "2677660",
    "2680010",
    "2686630",
    "2688950",
    "2692990",
    "2694490",
    "2698470",
    "2698780",
    "2698870",
    "2698940",
    "2700240",
    "2701660",
    "2703850",
    "2706300",
    "2732960",
    "2735580",
    "2737070",
    "2738750",
    "2740280",
    "2745870",
    "2751000",
    "2753270",
    "2776940",
    "2784470",
    "2785200",
    "2787320",
    "2800080",
    "2807960",
    "2810780",
    "2821610",
    "2825530",
    "2827200",
    "2827750",
    "2835530",
    "2840770",
    "2842040",
    "2842590",
    "2852190",
    "2853730",
    "2863640",
    "2864560",
    "2868840",
    "2878960",
    "2878980",
    "2881650",
    "2887520",
    "2893570",
    "2893820",
    "2897610",
    "2904000",
    "2909400",
    "2909780",
    "2916430",
    "2928600",
    "2933130",
    "2933620",
    "2934190",
    "2947440",
    "2950840",
    "2958130",
    "2961530",
    "2966850",
    "2976900",
    "3008130",
    "3008670",
    "3014080",
    "3014520",
    "3017860",
    "3021100",
    "3024040",
    "3025590",
    "3040220",
    "3041230",
    "3043580",
    "3045200",
    "3059070",
    "3061570",
    "3065800",
    "3065810",
    "3067890",
    "3070070",
    "3077390",
    "3097560",
    "3098140",
    "3110070",
    "3113680",
    "3125250",
    "3125570",
    "3126530",
    "3135840",
    "3136330",
    "3158270",
    "3162920",
    "3164500",
    "3165720",
    "3167970",
    "3168600",
    "3169520",
    "3173560",
    "3179730",
    "3180070",
    "3183280",
    "3201910",
    "3204110",
    "3209480",
    "3218530",
    "3219410",
    "3228590",
    "3230400",
    "3232040",
    "3232610",
    "3238670",
    "3240220",
    "3241660",
    "3258290",
    "3263320",
    "3265250",
    "3265700",
    "3272980",
    "3274780",
    "3278760",
    "3280350",
    "3285500",
    "3288210",
    "3288920",
    "3290440",
    "3293260",
    "3298460",
    "3314070",
    "3319120",
    "3321460",
    "3330160",
    "3352240",
    "3357650",
    "3362660",
    "3378960",
    "3393070",
    "3400930",
    "3405340",
    "3405690",
    "3408570",
    "3410490",
    "3419430",
    "3419520",
    "3430450",
    "3431040",
    "3431220",
    "3432970",
    "3438990",
    "3440120",
    "3446200",
    "3447690",
    "3450310",
    "3472040",
    "3474900",
    "3477300",
    "3489700",
    "3527290",
    "3551340",
    "3556750",
    "3558720",
    "3590330",
    "3595230",
    "3595270",
    "3607440",
    "3612060",
    "3616550",
    "3617930",
    "3634520",
    "3645890",
    "3649580",
    "3654560",
    "3655390",
    "3655960",
    "3657210",
    "3659180",
    "3668080",
    "3668370",
    "3672540",
    "3721890",
    "3739730",
    "3755860",
    "3756010",
    "3764200",
    "3768760",
    "3797340",
    "3800340",
    "3805710",
    "3814830",
    "3815050",
    "3819640",
    "3845440",
    "3882020",
    "3919190",
    "3925760",
    "3932890",
    "3942480",
    "3949040",
    "4015530",
    "4147110",
];


#[derive(Deserialize)]
struct SteamAppListResponse {
    applist: SteamAppList,
}

#[derive(Deserialize)]
struct SteamAppList {
    apps: Vec<SteamAppItem>,
}

#[derive(Deserialize)]
struct SteamAppItem {
    appid: Option<u32>,
    name: Option<String>,
}

pub struct RustGameManager {
    client: Client,
}

#[allow(dead_code)]
impl RustGameManager {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .user_agent("RagnarokLauncher/1.0 (Windows)")
            .connection_verbose(true)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }

    async fn do_fetch(client: &reqwest::Client, url: &str, key: Option<&str>, timeout_secs: u64) -> Result<Vec<SteamToolsGame>, String> {
        use std::time::Duration;
        let mut req = client
            .get(url)
            .timeout(Duration::from_secs(timeout_secs))
            .header("Accept", "application/json");
        if let Some(k) = key {
            req = req.header("X-Auth-Key", k);
        }
        let resp = req.send().await.map_err(|e| format!("Error de red: {}", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("HTTP {}", status));
        }
        let text = resp.text().await.map_err(|e| format!("Error leyendo respuesta: {}", e))?;
        serde_json::from_str::<Vec<SteamToolsGame>>(&text)
            .map_err(|e| format!("Error parseando JSON: {}", e))
    }

    /// Fetch catalog using curl.exe as subprocess (bypasses firewall blocks).
    async fn fetch_via_curl(url: &str, key: Option<&str>, timeout_secs: u64) -> Result<Vec<SteamToolsGame>, String> {
        let mut cmd = std::process::Command::new("curl.exe");
        // --compressed is not an optimisation here, it is the difference
        // between working and not: the catalog is 31.5 MB raw and 4.27 MB
        // gzipped, and the server already offers gzip. Without it a slow link
        // cannot finish the download inside --max-time, and curl hands back a
        // body cut off mid-JSON, which then fails to parse — exactly what a
        // user on Starlink reported, with the truncated payload visible in the
        // error message.
        cmd.args(["-sS", "-L", "--compressed", "--max-time", &timeout_secs.to_string(), url])
            .arg("-H")
            .arg("Accept: application/json")
            .creation_flags(0x08000000); // CREATE_NO_WINDOW — this fallback was flashing a
                                          // visible console window (multiple Discord reports:
                                          // "a CMD with curl.exe opens") every time the primary
                                          // reqwest fetch failed and it dropped to this path.
        if let Some(k) = key {
            cmd.arg("-H").arg(format!("X-Auth-Key: {}", k));
        }

        let output = cmd.output().map_err(|e| format!("Error ejecutando curl.exe: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(format!("curl.exe falló (exit {:?}): stderr=[{}] stdout=[{}]",
                output.status.code(), stderr.trim(), stdout.trim()));
        }
        serde_json::from_slice::<Vec<SteamToolsGame>>(&output.stdout)
            .map_err(|e| format!("Error parseando JSON de curl: {}", e))
    }

    /// Resolves a hostname via Cloudflare's DNS-over-HTTPS JSON API instead of
    /// the OS/ISP resolver. Some ISPs (Argentine users reported this
    /// specifically for generator.ryuu.lol, reliably fixed by turning on a
    /// VPN once) block or poison DNS for specific domains at the resolver
    /// level rather than blocking the IP itself — querying 1.1.1.1 directly
    /// (a literal IP, nothing to poison) sidesteps that without needing a
    /// full VPN.
    async fn resolve_via_doh(host: &str) -> Result<std::net::IpAddr, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|e| e.to_string())?;
        let resp: serde_json::Value = client
            .get(format!("https://1.1.1.1/dns-query?name={}&type=A", host))
            .header("Accept", "application/dns-json")
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

        resp["Answer"]
            .as_array()
            .and_then(|answers| {
                answers.iter().find_map(|a| {
                    // type 1 = A record
                    if a["type"].as_u64() == Some(1) {
                        a["data"].as_str().and_then(|ip| ip.parse().ok())
                    } else {
                        None
                    }
                })
            })
            .ok_or_else(|| "DoH no devolvió ningún registro A".to_string())
    }

    /// Try to fetch the game catalog from a given API URL.
    /// First with reqwest, then with curl.exe si falla (firewall block).
    async fn try_fetch_catalog(&self, url: &str, key: Option<&str>, timeout_secs: u64) -> Result<Vec<SteamToolsGame>, String> {
        use std::time::Duration;

        // 1. Cliente normal del manager (con TLS del sistema)
        match Self::do_fetch(&self.client, url, key, timeout_secs).await {
            Ok(items) => return Ok(items),
            Err(e) => {
                diag_log!("[try_fetch_catalog] Falló reqwest: {}. Reintentando con curl.exe...", e);
            }
        }

        // 2. Cliente con certificados no verificados (antivirus/proxy intercepta TLS)
        //
        // This is a real downgrade: the catalog for this one request is no
        // longer authenticated, so whoever is intercepting the connection
        // decides what game list arrives. It stays because the users who need
        // it — corporate/AV TLS interception, where the presented certificate
        // genuinely is untrusted — otherwise get no catalog at all, and
        // because it is scoped to public game metadata and nothing else. It
        // is recorded rather than silent, which it was not before.
        diag_log!("[try_fetch_catalog] Usando TLS sin verificar para {} — la respuesta no está autenticada.", url);
        let fallback_client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent("RagnarokLauncher/1.0 (Windows)")
            .danger_accept_invalid_certs(true)
            .no_proxy()
            .build()
        {
            Ok(c) => c,
            Err(e) => return Err(format!("Error creando cliente TLS no verificado: {}", e)),
        };

        match Self::do_fetch(&fallback_client, url, key, timeout_secs).await {
            Ok(items) => return Ok(items),
            Err(e) => {
                diag_log!("[try_fetch_catalog] También falló TLS permisivo: {}. Probando con curl.exe...", e);
            }
        }

        // 3. curl.exe como subproceso (bypassea firewall que bloquea sockets de la app)
        diag_log!("[try_fetch_catalog] Intentando con curl.exe...");
        let curl_err = match Self::fetch_via_curl(url, key, timeout_secs).await {
            Ok(items) => return Ok(items),
            Err(e) => e,
        };

        // 4. Resolución vía DNS-over-HTTPS (bloqueo/envenenamiento de DNS del ISP)
        if let Some(host) = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .and_then(|rest| rest.split('/').next())
        {
            diag_log!("[try_fetch_catalog] Todo falló. Intentando resolver '{}' vía DoH (posible bloqueo de DNS del ISP)...", host);
            match Self::resolve_via_doh(host).await {
                Ok(ip) => {
                    match reqwest::Client::builder()
                        .timeout(Duration::from_secs(timeout_secs))
                        .user_agent("RagnarokLauncher/1.0 (Windows)")
                        .resolve(host, std::net::SocketAddr::new(ip, 443))
                        .build()
                    {
                        Ok(doh_client) => {
                            if let Ok(items) = Self::do_fetch(&doh_client, url, key, timeout_secs).await {
                                diag_log!("[try_fetch_catalog] Éxito vía DoH ({} -> {})", host, ip);
                                return Ok(items);
                            }
                        }
                        Err(e) => diag_log!("[try_fetch_catalog] No se pudo crear cliente DoH: {}", e),
                    }
                }
                Err(e) => diag_log!("[try_fetch_catalog] DoH también falló: {}", e),
            }
        }

        Err(curl_err)
    }

    /// Last-resort catalog source: a copy of the already-processed catalog
    /// published to the project's own GitHub repo (written by main.rs's
    /// publish_catalog_mirror_to_github, owner-only).
    ///
    /// The DoH step in try_fetch_catalog only rescues a DNS-level block. When
    /// an ISP blocks by IP or by SNI instead, resolving the name correctly
    /// changes nothing — the connection to generator.ryuu.lol still dies, and
    /// the only thing that helps is a completely different domain. Reported
    /// by Argentinian users: the catalog never loaded for them unless they
    /// turned a VPN on, while the rest of the app (which talks to
    /// api.github.com and Steam) kept working fine — which is exactly the
    /// signature of one specific host being blocked rather than a broken
    /// connection. GitHub and jsDelivr are safe fallbacks precisely because
    /// blocking them breaks far too much unrelated software to be practical.
    const CATALOG_MIRROR_URLS: &'static [&'static str] = &[
        "https://raw.githubusercontent.com/RagnarokManifests/games/main/catalog.json",
        "https://cdn.jsdelivr.net/gh/RagnarokManifests/games@main/catalog.json",
    ];

    /// Last line between a bad connection and an empty library.
    ///
    /// One 30-second attempt per mirror was not enough: the catalog is tens of
    /// megabytes and reqwest's timeout covers the whole response body, so a
    /// satellite or mobile link loses this on payload size rather than on the
    /// mirror being unavailable — reported by a user on Starlink whose Library
    /// showed only the games already installed. The escalating budget matches
    /// what the Ryuu attempts above already get.
    ///
    /// A mirror that answered 404 or another client error is dropped instead
    /// of retried: the file is not there, and waiting 90 more seconds to be
    /// told so again only delays the fallback.
    async fn try_fetch_catalog_mirror(&self, deadline: std::time::Instant) -> Option<Vec<GameMetadata>> {
        let mut pending: Vec<&str> = Self::CATALOG_MIRROR_URLS.to_vec();

        for timeout in [30u64, 90u64] {
            let mut retry_next: Vec<&str> = Vec::new();

            for url in pending {
                // Shares sync_catalog's overall budget: by the time the
                // mirrors are reached the user has already been waiting, and
                // a second full round of escalation on top of it is exactly
                // the "never finishes loading" the budget exists to stop.
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    diag_log!("[sync_catalog] Presupuesto agotado; no se prueban más mirrors.");
                    return None;
                }

                let res = self
                    .client
                    .get(url)
                    .timeout(std::time::Duration::from_secs(timeout).min(remaining))
                    .send()
                    .await;
                match res {
                    Ok(res) if res.status().is_success() => match res.json::<Vec<GameMetadata>>().await {
                        Ok(games) if !games.is_empty() => {
                            diag_log!("[sync_catalog] Catálogo cargado desde mirror: {} ({} juegos)", url, games.len());
                            return Some(games);
                        }
                        Ok(_) => diag_log!("[sync_catalog] Mirror {} devolvió una lista vacía", url),
                        // A body cut off mid-download surfaces here, not as a
                        // send() error — worth one more try with more room.
                        Err(e) => {
                            diag_log!("[sync_catalog] Mirror {} devolvió JSON inválido (timeout {}s): {}", url, timeout, e);
                            retry_next.push(url);
                        }
                    },
                    Ok(res) => {
                        diag_log!("[sync_catalog] Mirror {} respondió HTTP {}", url, res.status());
                        if res.status().is_server_error() {
                            retry_next.push(url);
                        }
                    }
                    Err(e) => {
                        diag_log!("[sync_catalog] Mirror {} falló (timeout {}s): {}", url, timeout, e);
                        retry_next.push(url);
                    }
                }
            }

            if retry_next.is_empty() {
                break;
            }
            pending = retry_next;
        }
        None
    }

    /// Syncs game catalog: fetches names instantly, then fetches all sizes in parallel.
    /// Writes to a sibling temp file and renames it into place.
    ///
    /// `fs::write` truncates the target first and then streams into it, so an
    /// app that closes mid-write leaves a half-file behind. That matters here
    /// more than usual: the catalog cache is 31 MB, and a truncated one takes
    /// out both the one-hour fast path and the offline fallback that exists so
    /// a dead connection does not empty the Library. Rename is atomic on the
    /// same filesystem, so a reader sees either the old file or the new one.
    ///
    /// Best-effort by design — the callers already treat a failed cache write
    /// as "no cache", which is exactly right.
    fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    pub async fn sync_catalog(&self, _github_url: &str, ryuu_api_key: Option<&str>) -> Result<Vec<GameMetadata>, String> {
        // ── 0. Check local disk cache ──────────────────────────────────────────
        let cache_file = std::env::temp_dir().join("ragnarok_catalog_cache_v6.json");
        if let Ok(metadata) = std::fs::metadata(&cache_file) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed.as_secs() < 3600 {
                        if let Ok(json) = std::fs::read_to_string(&cache_file) {
                            if let Ok(games) = serde_json::from_str::<Vec<GameMetadata>>(&json) {
                                if !games.is_empty() && games.len() > 500 {
                                    return Ok(games);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Delete old cache files to force fresh fetch
        let old_caches = [
            "ragnarok_catalog_cache_v4.json",
            "ragnarok_catalog_cache_v2.json",
            "ragnarok_catalog_cache.json",
        ];
        for old in old_caches {
            let _ = std::fs::remove_file(std::env::temp_dir().join(old));
        }

        let mut enriched_games: Vec<GameMetadata> = Vec::new();
        let key = ryuu_api_key.unwrap_or("RYUUMANIFESTfqdpr3");

        // ── Try Ryuu API con reintentos ────────────────────────────────────
        let mut fetch_success = false;
        let mut all_errors: Vec<String> = Vec::new();
        let mut empty_response_count = 0u32;

        // How long the whole sync may take, fallbacks included.
        //
        // Without a ceiling the retries multiply out of control. Each call to
        // try_fetch_catalog runs FOUR strategies in sequence — plain reqwest,
        // reqwest with permissive TLS, curl.exe, then DNS-over-HTTPS — and
        // hands every one of them the same timeout, so a "90 second" attempt
        // really costs about six minutes. The three escalating attempts
        // together come to roughly twelve, and only then do the mirrors start.
        //
        // A connection that is slow rather than broken burns every one of
        // those timeouts instead of failing fast, which is what a user on
        // Starlink hit: the catalog never finished loading because it was
        // still, legitimately, trying. The escalation is worth keeping (a slow
        // link genuinely needs more than 30 seconds), so it is bounded rather
        // than removed — past the deadline the remaining strategies are
        // skipped and the cache fallback below takes over.
        const CATALOG_BUDGET_SECS: u64 = 180;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(CATALOG_BUDGET_SECS);

        'primary: for timeout in [30u64, 60u64, 90u64] {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                all_errors.push(format!("presupuesto total de {}s agotado", CATALOG_BUDGET_SECS));
                diag_log!("[sync_catalog] Presupuesto de {}s agotado; pasando a los respaldos.", CATALOG_BUDGET_SECS);
                break 'primary;
            }

            // Clamped so the last attempt cannot outlive the budget on its own.
            let attempt = self.try_fetch_catalog(
                "https://generator.ryuu.lol/api/games",
                Some(key),
                timeout.min(remaining.as_secs().max(1)),
            );

            match tokio::time::timeout(remaining, attempt).await {
                Ok(Ok(items)) if !items.is_empty() => {
                    diag_log!("[sync_catalog] Ryuu API OK: {} games desde generator.ryuu.lol", items.len());
                    fetch_success = true;
                    enriched_games = self.process_catalog_items(items);
                    break 'primary;
                }
                Ok(Ok(_)) => {
                    empty_response_count += 1;
                    diag_log!("[sync_catalog] API devolvió lista vacía");
                }
                Ok(Err(e)) => {
                    all_errors.push(format!("(timeout {}s) {}", timeout, e));
                    diag_log!("[sync_catalog] Falló generator.ryuu.lol (timeout {}s): {}", timeout, e);
                }
                Err(_) => {
                    all_errors.push(format!("presupuesto total de {}s agotado", CATALOG_BUDGET_SECS));
                    diag_log!("[sync_catalog] Presupuesto de {}s agotado a mitad del intento.", CATALOG_BUDGET_SECS);
                    break 'primary;
                }
            }
        }

        // Ryuu unreachable (or answering with nothing usable) — fall back to
        // the GitHub-hosted mirror before giving up. See
        // try_fetch_catalog_mirror's doc comment for why a different domain
        // is the only thing that helps some users.
        if !fetch_success || enriched_games.is_empty() {
            if let Some(mirror_games) = self.try_fetch_catalog_mirror(deadline).await {
                enriched_games = mirror_games;
                fetch_success = true;
            }
        }

        // Everything on the network failed. The one-hour gate at the top of
        // this function exists to keep the catalog fresh, not to throw it away
        // — a catalog from last week beats the error screen, which is what a
        // user actually sees: the Library falls back to just the games already
        // installed (reported as "4 juegos totales, 4 instalados"). Age is
        // therefore ignored here, unlike in the fast path above.
        if !fetch_success || enriched_games.is_empty() {
            if let Ok(json) = std::fs::read_to_string(&cache_file) {
                if let Ok(games) = serde_json::from_str::<Vec<GameMetadata>>(&json) {
                    if games.len() > 500 {
                        diag_log!(
                            "[sync_catalog] Red caída; sirviendo el catálogo cacheado ({} juegos).",
                            games.len()
                        );
                        return Ok(games);
                    }
                }
            }
        }

        if !fetch_success || enriched_games.is_empty() {
            // The generic "check your internet" message was swallowing the
            // real cause — every attempt's outcome was tracked but never
            // surfaced, so a genuinely different problem (e.g. the server
            // responding successfully but with zero games, most likely from
            // an invalid/rate-limited custom Ryuu API key set in Settings)
            // looked identical to an actual network failure. Append what
            // really happened so it doesn't need a live debugging session
            // to diagnose next time.
            let detail = if empty_response_count > 0 && all_errors.is_empty() {
                format!(
                    "El servidor de Ryuu respondió correctamente {} vez(es) pero sin juegos — no parece ser un problema de conexión. Si tienes una API Key de Ryuu personalizada en Ajustes, revisa que sea válida.",
                    empty_response_count
                )
            } else if !all_errors.is_empty() {
                format!("Detalle técnico: {}", all_errors.join(" | "))
            } else {
                "Motivo desconocido.".to_string()
            };
            return Err(format!(
                "Error de conexión: No se pudo cargar el catálogo. Revisa tu conexión a internet o intenta más tarde. [{}]",
                detail
            ));
        }

        enriched_games.sort_by(|a, b| {
            let a_new = a.is_new.unwrap_or(false);
            let b_new = b.is_new.unwrap_or(false);
            match (b_new, a_new) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => a.name.cmp(&b.name),
            }
        });

        if !enriched_games.is_empty() {
            if let Ok(json) = serde_json::to_string(&enriched_games) {
                let _ = Self::write_atomic(&cache_file, json.as_bytes());
                crate::managers::hubcap::remember_adult_ids(&enriched_games);
            }
        }

        Ok(enriched_games)
    }

    fn process_catalog_items(&self, items: Vec<SteamToolsGame>) -> Vec<GameMetadata> {
        let known_ids_file = std::env::var("APPDATA")
            .map(|p| std::path::PathBuf::from(p).join("Ragnarok Launcher").join("known_game_ids.json"))
            .unwrap_or_else(|_| std::env::temp_dir().join("ragnarok_known_game_ids.json"));

        let value_to_appid = |v: &serde_json::Value| -> String {
            match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => String::new(),
            }
        };

        #[derive(Deserialize, Serialize, Clone, Debug)]
        #[serde(untagged)]
        enum KnownIdsFormat {
            Map(std::collections::HashMap<String, u64>),
            Array(Vec<String>),
        }

        // A file we cannot read has to mean the same as no file at all.
        //
        // These two branches used to disagree. A missing file seeded every
        // appid with 0 — "seen before, not new" — while an unparseable one
        // produced an empty map, and the loop below then stamped all 69,560
        // ids with the current time. Every game in the catalog came back
        // flagged NEW, the sort piled them into one block, and it fixed itself
        // only after 24 hours.
        //
        // Reaching that state took nothing exotic: the write below is not
        // atomic, so closing the app mid-write leaves truncated JSON.
        let seed_as_known = || -> std::collections::HashMap<String, u64> {
            items.iter().map(|item| (value_to_appid(&item.appid), 0)).collect()
        };

        let mut known_map: std::collections::HashMap<String, u64> = match std::fs::read_to_string(&known_ids_file) {
            Ok(json) => match serde_json::from_str::<KnownIdsFormat>(&json) {
                Ok(KnownIdsFormat::Map(map)) => map,
                Ok(KnownIdsFormat::Array(arr)) => arr.into_iter().map(|id| (id, 0)).collect(),
                Err(e) => {
                    diag_log!("[sync_catalog] known_game_ids.json ilegible ({}); se trata como primera vez.", e);
                    seed_as_known()
                }
            },
            Err(_) => seed_as_known(),
        };

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        for item in &items {
            let appid_str = value_to_appid(&item.appid);
            if !known_map.contains_key(&appid_str) {
                known_map.insert(appid_str, current_time);
            }
        }

        if let Ok(json) = serde_json::to_string(&known_map) {
            if let Some(parent) = known_ids_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = Self::write_atomic(&known_ids_file, json.as_bytes());
        }

        let mut games = Vec::new();
        for item in items {
            let appid_str = value_to_appid(&item.appid);
            if appid_str.is_empty() { continue; }

            let is_new = if let Some(&discovery_time) = known_map.get(&appid_str) {
                discovery_time > 0 && (current_time.saturating_sub(discovery_time) < 86400)
            } else {
                false
            };

            let image = item.header_image.unwrap_or_else(|| {
                format!("https://cdn.akamai.steamstatic.com/steam/apps/{}/header.jpg", appid_str)
            });

            games.push(GameMetadata {
                id: appid_str,
                name: item.name,
                image: Some(image),
                developers: None,
                genres: if item.tags.is_empty() { None } else { Some(item.tags) },
                metacritic: None,
                status: None,
                size_bytes: None,
                is_new: if is_new { Some(true) } else { None },
                updated_at: item.updated_date,
                nsfw: if item.nsfw { Some(true) } else { None },
                added_at: item.added_date,
            });
        }
        games
    }

    /// Helper to fetch names from a reliable backup mirror if Steam API is acting up
    pub async fn fetch_backup_name(&self, app_id: &str) -> Option<String> {
        let url = format!("https://store.steampowered.com/api/appdetails?appids={}&filters=basic", app_id);
        if let Ok(res) = self.client.get(url).send().await {
            if let Ok(json) = res.json::<serde_json::Value>().await {
                if let Some(name) = json[app_id]["data"]["name"].as_str() {
                    return Some(name.to_string());
                }
            }
        }
        None
    }

    /// Maps a numeric Steam genre ID to its human-readable name.
    fn steam_genre_name(id: &str) -> Option<&'static str> {
        match id.trim() {
            "1"  => Some("Action"),
            "2"  => Some("Strategy"),
            "3"  => Some("RPG"),
            "4"  => Some("Casual"),
            "5"  => Some("Racing"),
            "6"  => Some("Sports"),
            "7"  => Some("Adventure"),
            "9"  => Some("Massively Multiplayer"),
            "11" => Some("Free to Play"),
            "18" => Some("Sports"),
            "23" => Some("Indie"),
            "25" => Some("Adventure"),
            "28" => Some("Simulation"),
            "29" => Some("Racing"),
            "37" => Some("Free to Play"),
            "51" => Some("Animation & Modeling"),
            "52" => Some("Video Production"),
            "53" => Some("Photo Editing"),
            "54" => Some("Utilities"),
            "55" => Some("Game Development"),
            "56" => Some("Education"),
            "57" => Some("Software Training"),
            "58" => Some("Web Publishing"),
            "59" => Some("Audio Production"),
            "70" => Some("Early Access"),
            _ => None,
        }
    }

    /// Static version so it can be moved into tokio::spawn without self.
    /// Returns (name, size, genres)
    async fn fetch_steamcmd_info_static(client: Client, app_id: String) -> (Option<String>, Option<u64>, Option<Vec<String>>) {
        let url = format!("https://api.steamcmd.net/v1/info/{}", app_id);
        let data: Option<serde_json::Value> = async {
            client.get(&url).send().await.ok()?.json().await.ok()
        }.await;

        let mut name = None;
        let mut size = None;
        let mut genres: Option<Vec<String>> = None;

        if let Some(json) = data {
            if let Some(app_data) = json["data"][&app_id].as_object() {
                // Try to get real name from SteamCMD
                if let Some(n) = app_data.get("common").and_then(|c| c.get("name")).and_then(|n| n.as_str()) {
                    let n = n.trim();
                    if !n.is_empty() && !n.to_lowercase().starts_with("app ") {
                        name = Some(n.to_string());
                    }
                }

                // Try to get genres from SteamCMD common.genres
                // Values are either genre name strings OR numeric Steam genre IDs
                if let Some(genre_obj) = app_data.get("common").and_then(|c| c.get("genres")).and_then(|g| g.as_object()) {
                    let genre_list: Vec<String> = genre_obj
                        .values()
                        .filter_map(|v| v.as_str())
                        .filter_map(|s| {
                            // If it's a numeric ID, resolve to name; otherwise use as-is
                            if s.chars().all(|c| c.is_ascii_digit()) {
                                Self::steam_genre_name(s).map(|n| n.to_string())
                            } else if !s.is_empty() {
                                Some(s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect::<std::collections::HashSet<_>>() // deduplicate (e.g. Sports = 6 and 18)
                        .into_iter()
                        .collect();
                    if !genre_list.is_empty() {
                        genres = Some(genre_list);
                    }
                }

                // Try to get size
                if let Some(depots) = app_data.get("depots").and_then(|d| d.as_object()) {
                    let mut total: u64 = 0;
                    for (depot_id, info) in depots {
                        if !depot_id.chars().all(|c| c.is_ascii_digit()) {
                            continue;
                        }
                        if let Some(v) = info["maxsize"]
                            .as_str()
                            .and_then(|s| s.parse::<u64>().ok())
                            .or_else(|| info["maxsize"].as_u64())
                        {
                            total += v;
                        }
                    }
                    if total > 0 {
                        size = Some(total);
                    }
                }
            }
        }

        // Ultimate Fallback: Try SteamSpy API if name or genres are still missing
        if name.is_none() || genres.is_none() {
            let spy_url = format!("https://steamspy.com/api.php?request=appdetails&appid={}", app_id);
            if let Ok(res) = client.get(&spy_url).send().await {
                if let Ok(spy_data) = res.json::<serde_json::Value>().await {
                    if name.is_none() {
                        if let Some(n) = spy_data.get("name").and_then(|n| n.as_str()) {
                            let n = n.trim();
                            if !n.is_empty() && !n.to_lowercase().starts_with("app ") {
                                name = Some(n.to_string());
                            }
                        }
                    }
                    // SteamSpy has genre as a comma-separated string in "genre" field
                    if genres.is_none() {
                        if let Some(genre_str) = spy_data.get("genre").and_then(|g| g.as_str()) {
                            let genre_list: Vec<String> = genre_str
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            if !genre_list.is_empty() {
                                genres = Some(genre_list);
                            }
                        }
                    }
                }
            }
        }

        (name, size, genres)
    }

    pub fn save_cache(&self, games: Vec<GameMetadata>, path: PathBuf) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&games).map_err(|e| e.to_string())?;
        fs::write(path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load_cache(&self, path: PathBuf) -> Result<Vec<GameMetadata>, String> {
        let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let games: Vec<GameMetadata> = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        Ok(games)
    }

}

