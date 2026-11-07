//! The command tree: every area, verb, argument and help text of `truesense`, declared once.
//!
//! The parser, the `-h` pages, the error messages and the shell completions all read this tree, so
//! they cannot disagree. The words are those of the Python edition (`cli.py`), whose help pages
//! are the reference output of the tests.
#![forbid(unsafe_code)]

use ts_domain::perkeyfx;
use ts_platform::hw::{Coded, GpuMode, PowerMode, RearMode};

use super::argparse::{Action, Arg, Command};

/// The pages `gui --page` accepts (the Python window had these four).
pub const GUI_PAGES: [&str; 4] = ["home", "keys", "audio", "settings"];

/// The pages `tui --page` accepts, in the order of the tabs.
pub const TUI_PAGES: [&str; 7] = ["dashboard", "fans", "power", "lighting", "audio", "profiles", "system"];

const TRI: [&str; 4] = ["on", "off", "toggle", "status"];
const PROFILE_VERBS: [&str; 7] = ["list", "current", "show", "use", "copy", "save", "delete"];
const GESTURES: [&str; 5] = ["tap", "double", "triple", "long", "fn_esc"];

fn names<T: Coded>() -> Vec<String> {
    T::ALL.iter().map(|m| m.name().to_owned()).collect()
}

fn verb(name: &str) -> Command {
    Command::new(name)
}

/// A positional that takes `on off toggle status` (or the given words), defaulting to `status`.
fn switch(words: &[&str]) -> Arg {
    Arg::pos("action").optional().default_str("status").choices(words)
}

fn brightness() -> Arg {
    Arg::opt("--brightness").int()
}

fn confirm(help: &str) -> Arg {
    Arg::flag("--confirm").help(help)
}

fn force() -> Arg {
    Arg::flag("--force").help("allow values beyond the danger line")
}

fn profile_actions(first_default: &str) -> Arg {
    Arg::pos("action").optional().default_str(first_default).choices(&PROFILE_VERBS)
}

/// A subcommand with a switch argument and no other option.
fn tri_verb(name: &str) -> Command {
    verb(name).arg(switch(&TRI))
}

/// The whole program.
pub(super) fn build(version: &str) -> Command {
    let areas = vec![
        verb("status").help("everything at a glance"),
        verb("sensors").help("temperatures and fan RPM"),
        power(),
        super::modebind::command(),
        fan(),
        battery(),
        verb("display").help("LCD overdrive").subs("verb", vec![tri_verb("overdrive")]),
        verb("boot").help("boot animation and sound").subs("verb", vec![tri_verb("sound")]),
        gpu(),
        rear(),
        update(),
        look(),
        startup(),
        keyboard(),
        audio(),
        keys(),
        profile(),
        super::favourite::command(),
        super::themes::command(),
        cpu(),
        tri_verb("touchpad").help("touchpad on/off (Fn+F10)"),
        super::alerts::command(),
        service(),
        verb("config").help("configuration").subs("verb", vec![verb("show"), verb("validate"), verb("path")]),
        verb("doctor").help("check the whole stack and say how to fix problems"),
        verb("selftest")
            .help("set -> read back -> restore every firmware feature")
            .arg(Arg::flag("--skip-fans")),
        verb("completion")
            .help("shell completion script")
            .arg(Arg::pos("shell").choices(&["bash", "zsh", "fish"])),
        verb("gui")
            .help("the small optional window")
            .arg(Arg::opt("--page").choices(&GUI_PAGES).help("open on this page")),
        verb("probe").help("read-only report of this laptop for model support (never writes a setting)"),
        verb("tui")
            .help("the full control surface in the terminal (keyboard and mouse)")
            .arg(Arg::opt("--page").choices(&TUI_PAGES).help("open on this page")),
        verb("daemon").help("run the daemon (systemd does this)"),
        verb("version"),
    ];
    let mut root = Command::new("truesense")
        .arg(Arg::flag("--json").help("machine-readable output"))
        .subs("area", areas)
        .subs_metavar("area");
    let mut version_arg = Arg::flag("--version").help("show program's version number and exit");
    version_arg.flags = vec!["-V".to_owned(), "--version".to_owned()];
    version_arg.action = Action::Version(format!("truesense {version}"));
    root.args.push(version_arg);
    root.description = Some(
        "truesense: command line for the Acer Predator Helios 16 (PH16-71).   truesense <area> <verb> ...   (every setting has on / off / toggle / status; --json for scripts)"
            .to_owned(),
    );
    root.epilog = Some(
        "Examples:\n  truesense status\n  truesense power set turbo\n  truesense fan set 60\n  truesense rear set breathing --color 00ff88\n  truesense keyboard effect wave --brightness 50\n  truesense keys set double output-toggle\n  truesense profile apply gaming\n  truesense doctor"
            .to_owned(),
    );
    root
}

fn power() -> Command {
    let modes = names::<PowerMode>();
    verb("power")
        .help("power mode (quiet balanced performance turbo eco) and the automatic profile")
        .subs(
            "verb",
            vec![
                verb("status"),
                verb("list"),
                verb("set").arg(Arg::pos("mode").choices_owned(modes.clone())),
                verb("next"),
                verb("prev"),
                verb("smart")
                    .help("Smart auto profiles: list | current | show NAME | use NAME | copy NAME [NEW] | save NAME [--from BASE] --set key=value ... | delete NAME (built-in profiles are read-only: copy them to edit)")
                    .arg(profile_actions("current"))
                    .arg(Arg::pos("name").optional())
                    .arg(Arg::pos("new").optional())
                    .arg(Arg::opt("--from").dest("base"))
                    .arg(Arg::append("--set").metavar("KEY=VALUE")),
                verb("auto")
                    .help("automatic power mode: off | native (PredatorSense-like: power source only) | smart (heat, load and battery aware); on = smart")
                    .arg(switch(&["off", "native", "smart", "on", "toggle", "status"])),
                verb("hold")
                    .help("keep the current power mode against the automatic profile")
                    .arg(Arg::pos("minutes").optional().float().default_float(30.0)),
                verb("release").help("end a hold"),
                verb("managers")
                    .help("other power managers (auto-cpufreq, TLP, tuned, power-profiles-daemon, system76-power): list | stop (TrueSense manages power: stops them, sets sane CPU defaults, remembers) | restore")
                    .arg(Arg::pos("action").optional().default_str("list").choices(&["list", "stop", "restore"])),
                verb("game")
                    .help("game rules: a program raises the power mode while it runs: list | add NAME MODE | remove NAME | clear")
                    .arg(Arg::pos("action").optional().default_str("list").choices(&["list", "add", "remove", "clear"]))
                    .arg(Arg::pos("name").optional())
                    .arg(Arg::pos("mode").optional().choices_owned(modes)),
            ],
        )
}

fn fan() -> Command {
    verb("fan").help("fans: firmware auto, max, manual %, smart curve").subs(
        "verb",
        vec![
            verb("status"),
            verb("auto").help("firmware control (0 rpm when cool)"),
            verb("max"),
            verb("off")
                .help("stop both fans (needs --confirm; only when cool; the service returns them to Auto if it gets warm)")
                .arg(confirm("I understand the laptop can overheat with the fans off")),
            verb("set")
                .help("manual duty per fan, 20..100 %, or 0 to stop that fan (needs --confirm, only when cool)")
                .arg(Arg::pos("cpu").int())
                .arg(Arg::pos("gpu").int().optional())
                .arg(confirm("I understand a stopped fan can let the laptop overheat")),
            verb("curve")
                .help("smart-fan curves: list | current | show NAME | use NAME | copy NAME [NEW] | save NAME --cpu PTS --gpu PTS | delete NAME  (built-in curves are read-only: copy them to edit)")
                .arg(Arg::pos("action").optional().default_str("current").choices(&["list", "current", "show", "use", "save", "delete", "copy"]))
                .arg(Arg::pos("name").optional())
                .arg(Arg::pos("new").optional().help("copy: the new name"))
                .arg(Arg::opt("--cpu").help("CPU points, e.g. 52:0,60:35,70:55,80:75,89:100 (temperature:percent, 0 = firmware auto)"))
                .arg(Arg::opt("--gpu").help("GPU points"))
                .arg(Arg::opt("--hysteresis").float())
                .arg(Arg::opt("--down-rate").float())
                .arg(Arg::opt("--predict").float())
                .arg(Arg::opt("--couple").float())
                .arg(Arg::opt("--battery-shift").float())
                .arg(Arg::opt("--from").dest("base").help("start from this curve and change only what you give")),
            verb("smart").help("the daemon's smart curve").arg(switch(&TRI)),
        ],
    )
}

fn battery() -> Command {
    verb("battery")
        .help("80 % charge limit, charge to full once, calibration, USB power-off charging")
        .subs(
            "verb",
            vec![
                verb("full")
                    .help("charge to 100 % once: the 80 % limit comes back by itself (unplug, full for 5 minutes, or 24 hours)")
                    .arg(switch(&["start", "cancel", "status"])),
                tri_verb("limit"),
                tri_verb("calibration"),
                verb("usb-charge")
                    .help("USB charging while powered off: stop level 0 (off) 10 20 30")
                    .arg(Arg::pos("level").optional().default_str("status").choices(&["status", "0", "10", "20", "30"])),
                verb("status"),
            ],
        )
}

fn gpu() -> Command {
    verb("gpu").help("GPU mode (MUX); a change needs a reboot").subs(
        "verb",
        vec![
            verb("status"),
            verb("set").arg(Arg::pos("mode").choices_owned(names::<GpuMode>())),
            verb("undervolt")
                .help("GPU undervolt profiles (clock-curve offset + clock cap; reset on reboot; applying needs --confirm): get | list | show NAME | use NAME --confirm | reset | copy NAME [NEW] | save NAME --offset MHZ [--cap MHZ] | delete NAME | boot NAME|off --confirm")
                .arg(Arg::pos("action").choices(&["get", "list", "show", "use", "reset", "copy", "save", "delete", "boot"]))
                .arg(Arg::pos("name").optional())
                .arg(Arg::pos("new").optional())
                .arg(Arg::opt("--offset").int())
                .arg(Arg::opt("--cap").int().default_int(0))
                .arg(confirm("I understand this can crash games or the driver"))
                .arg(force()),
        ],
    )
}

fn rear() -> Command {
    verb("rear").help("rear light bar").subs(
        "verb",
        vec![
            verb("status"),
            verb("on"),
            verb("off"),
            verb("toggle"),
            verb("set")
                .arg(Arg::pos("effect").choices_owned(names::<RearMode>()))
                .arg(Arg::opt("--color").default_str("ffffff"))
                .arg(Arg::opt("--speed").int().default_int(3))
                .arg(Arg::opt("--brightness").int().default_int(100))
                .arg(Arg::opt("--direction").int().default_int(1).choices(&["0", "1"]))
                .arg(Arg::opt("--zones").help(
                    "static only: one colour, or three (left,middle,right), e.g. ff0000,00ff00,0000ff",
                )),
        ],
    )
}

fn update() -> Command {
    verb("update").help("check for a newer TrueSense and install it").subs(
        "verb",
        vec![
            verb("check")
                .help("ask GitHub for the newest release")
                .arg(Arg::flag("--force").help("ignore the daily cache")),
            verb("apply")
                .help("download the newest release and run its installer (asks for administrator rights)")
                .arg(Arg::flag("--yes").help("do not ask first"))
                .arg(Arg::opt("--version").help("install this version instead of the newest, e.g. 0.3.1")),
            verb("auto")
                .help("check for updates by itself once a day (a desktop notification, nothing is installed)")
                .arg(switch(&TRI)),
            verb("auto-run").help_suppressed(),
        ],
    )
}

fn look() -> Command {
    verb("look").help("saved lighting looks (keyboard effect/brightness/colours + rear light)").subs(
        "verb",
        vec![
            verb("list"),
            verb("save").help("save what is on the lights now").arg(Arg::pos("name")),
            verb("apply").arg(Arg::pos("name")),
            verb("show").arg(Arg::pos("name")),
            verb("delete").arg(Arg::pos("name")),
        ],
    )
}

fn startup() -> Command {
    verb("startup").help("what the lights and profile do at login").subs(
        "verb",
        vec![
            verb("show"),
            verb("apply").help("do it now (the user service does this at login)"),
            verb("boot")
                .help("the look applied at BOOT (before login) and after suspend: NAME | current | clear (no argument: show)")
                .arg(Arg::pos("what").optional()),
            verb("set")
                .arg(Arg::pos("target").choices(&["keyboard", "rear", "profile"]))
                .arg(Arg::pos("value").help("keyboard: last|default|none|off|profile:NAME   rear: last|default|none|off   profile: a profile name or none")),
        ],
    )
}

fn keyboard() -> Command {
    let mut fx_modes = vec!["off".to_owned()];
    fx_modes.extend(perkeyfx::mode_names().into_iter().map(str::to_owned));
    verb("keyboard").help("keyboard lighting").subs(
        "verb",
        vec![
            verb("status"),
            verb("on"),
            verb("off"),
            verb("toggle"),
            verb("restore").help("back to the stock cyan per-key state"),
            verb("list-keys"),
            verb("effect")
                .arg(Arg::pos("name"))
                .arg(Arg::opt("--color").default_str("00aec7"))
                .arg(Arg::opt("--speed").int().default_int(5))
                .arg(brightness())
                .arg(Arg::opt("--direction").help(
                    "wave only: left-to-right | right-to-left | bottom-to-top | top-to-bottom | clockwise | counter-clockwise, or 0..5 in that order (default: left-to-right)",
                )),
            verb("fx")
                .help("per-key animation on chosen keys (drawn by the key service, about 5 frames a second): off | pulse | wave | cycle | twinkle | rainbow | scanner | comet | ripple | rain | matrix | fire | plasma | aurora | lightning | confetti | starlight | mood | breathing | wave-clockwise | wave-counter-clockwise")
                .arg(Arg::pos("mode").choices_owned(fx_modes))
                .arg(Arg::opt("--speed").int().default_int(5).help("1..10"))
                .arg(Arg::opt("--keys").help("comma list of keys or groups, e.g. wasd,arrows,f5 (default: all keys)")),
            verb("speed")
                .help("animation speed of the running effect, 1 (slowest) .. 12 (fastest)")
                .arg(Arg::pos("level").int()),
            verb("color")
                .help("change the colour of the current effect (stays in the same mode)")
                .arg(Arg::pos("color")),
            verb("paint")
                .help("colour some keys and keep the rest, e.g. paint w=ff0000 arrows=00ff00 esc=ffffff")
                .arg(Arg::pos("items").plus())
                .arg(brightness()),
            verb("layout").help("key names and positions (for other tools)"),
            verb("design")
                .help("still designs: list | apply NAME | save NAME (yours go in ~/.config/truesense/designs/NAME.json)")
                .arg(Arg::pos("action").choices(&["list", "apply", "save"]))
                .arg(Arg::pos("name").optional())
                .arg(brightness()),
            verb("image")
                .help("paint a picture across the keys (needs Pillow)")
                .arg(Arg::pos("file"))
                .arg(brightness()),
            verb("fill").arg(Arg::pos("color")).arg(brightness()),
            verb("gradient").arg(Arg::pos("from_color")).arg(Arg::pos("to_color")).arg(brightness()),
            verb("rainbow").arg(brightness()),
            verb("keys")
                .help("per-key colours, e.g. keys \"wasd=ff0000\" \"esc=00ff00\" --base 101010")
                .arg(Arg::pos("items").plus())
                .arg(Arg::opt("--base").default_str("000000"))
                .arg(brightness()),
            verb("brightness")
                .help("0..50 (also Fn+F11/F12)")
                .arg(Arg::pos("level").optional().int()),
            verb("timeout").help("backlight off after 30 s of no typing").arg(switch(&TRI)),
            verb("operating").help("colour follows the power mode").arg(switch(&["on", "off", "status"])),
            verb("profile")
                .help("saved lighting profiles")
                .arg(Arg::pos("verb2").choices(&["list", "load", "save", "delete"]))
                .arg(Arg::pos("name").optional())
                .arg(Arg::pos("spec").star().help("save: fill COLOR | gradient C1 C2 | rainbow")),
        ],
    )
}

fn audio() -> Command {
    verb("audio").help("output, microphone, headset jack, Purified Voice, DTS").subs(
        "verb",
        vec![
            verb("output")
                .help("speakers / headphones while the aux jack is plugged in")
                .arg(switch(&["status", "speakers", "headphones", "toggle", "cycle"])),
            verb("mic")
                .arg(switch(&["status", "mute", "unmute", "toggle", "volume", "boost"]))
                .arg(Arg::pos("value").optional().int())
                .arg(Arg::flag("--default-only").help("act on the default input only (default: every input)")),
            verb("jack")
                .help("what the codec sees on the 3.5 mm jack; `jack headphones|headset|auto` sets how the jack is used")
                .arg(Arg::pos("use").optional().choices(&["auto", "headphones", "headset"])),
            verb("voice")
                .help("Purified Voice (AI mic noise reduction)")
                .arg(switch(&["status", "set", "enable", "disable", "next", "apply", "watch", "notify"]))
                .arg(Arg::pos("a").optional())
                .arg(Arg::pos("b").optional()),
            verb("repair").help("no sound? restart WirePlumber so it probes the sound card again, and check that an output exists"),
            verb("speaker-filter")
                .help("noise filter on the output (off by default: it also gates music)")
                .arg(switch(&["on", "off", "status"])),
            verb("dts")
                .help("Acer DTS sound through AcerUltraX")
                .arg(switch(&["status", "on", "off", "mode", "profiles", "devices", "all", "setup"]))
                .arg(Arg::pos("name").optional()),
        ],
    )
}

fn keys() -> Command {
    let mut profile = verb("profile")
        .help("key binding profiles: list | current | show NAME | use NAME | copy NAME [NEW] | save NAME [--from BASE] [--tap A --double A ...] | delete NAME (built-in profiles are read-only: copy to edit)")
        .arg(profile_actions("current"))
        .arg(Arg::pos("name").optional())
        .arg(Arg::pos("new").optional())
        .arg(Arg::opt("--from").dest("base"));
    for g in ["tap", "double", "triple", "long", "fn-esc"] {
        profile = profile.arg(Arg::opt(&format!("--{g}")));
    }
    profile = profile.arg(Arg::opt("--long-ms").int()).arg(Arg::opt("--multi-ms").int());
    verb("keys").help("PREDATOR key and Fn+Esc: what each gesture does").subs(
        "verb",
        vec![
            verb("show"),
            verb("actions"),
            verb("run").help("run the key service in the foreground (what the systemd user service does)"),
            verb("set")
                .arg(Arg::pos("gesture").choices(&GESTURES))
                .arg(Arg::pos("action").help("see: truesense keys actions")),
            verb("reset").arg(Arg::pos("gesture").optional().choices(&GESTURES)),
            verb("test").arg(Arg::pos("action").help("run an action now, as the key would")),
            profile,
            verb("timing").arg(Arg::opt("--long-ms").int()).arg(Arg::opt("--multi-ms").int()),
        ],
    )
}

fn profile() -> Command {
    verb("profile").help("named bundles of settings (gaming, quiet, battery-saver, yours)").subs(
        "verb",
        vec![
            verb("list"),
            verb("next"),
            verb("show").arg(Arg::pos("name")),
            verb("apply").arg(Arg::pos("name")),
            verb("save")
                .help("save the CURRENT state as a profile")
                .arg(Arg::pos("name"))
                .arg(Arg::opt("--description").default_str("")),
            verb("delete").arg(Arg::pos("name")),
        ],
    )
}

fn cpu() -> Command {
    verb("cpu").help("CPU power limits and the guarded undervolt").subs(
        "verb",
        vec![
            verb("info"),
            verb("limits")
                .help("CPU power limits PL1 (sustained) and PL2 (burst) in watts; no argument shows them; --auto goes back to automatic")
                .arg(Arg::pos("pl1").float().optional())
                .arg(Arg::pos("pl2").float().optional())
                .arg(Arg::flag("--auto"))
                .arg(force()),
            verb("undervolt")
                .help("CPU undervolt profiles (reset on reboot; applying needs --confirm): get | list | show NAME | use NAME --confirm | reset | copy NAME [NEW] | save NAME --core MV --cache MV | delete NAME | boot NAME|off --confirm")
                .arg(Arg::pos("action").choices(&["get", "list", "show", "use", "reset", "copy", "save", "delete", "boot", "set"]))
                .arg(Arg::pos("name").optional())
                .arg(Arg::pos("new").optional())
                .arg(Arg::opt("--core").int())
                .arg(Arg::opt("--cache").int())
                .arg(confirm("I understand this can freeze or reboot the laptop"))
                .arg(force())
                .arg(Arg::opt("--plane").int().default_int(0).choices(&["0", "2"]).help("set: 0 core, 2 cache"))
                .arg(Arg::pos("mv").int().optional().hidden()),
        ],
    )
}

fn service() -> Command {
    verb("service").help("the daemon").subs(
        "verb",
        vec![
            verb("status"),
            verb("restart"),
            verb("logs"),
            verb("check").help("every unit, the kernel module and the control socket (read-only)"),
            verb("repair")
                .help("get the daemon, its health timer, the module and your user services running again (safe to repeat; touches no setting)")
                .arg(Arg::flag("--system").hidden()),
        ],
    )
}
