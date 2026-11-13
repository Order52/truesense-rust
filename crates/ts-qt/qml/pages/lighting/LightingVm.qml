// What the Lighting page's parts share: the call into the Rust view-model (ts-ui-model/src/vm/
// lighting.rs), the key picture (read once from `keyboard layout`), the list of looks, and the
// words for effects. A singleton, so it outlives the page (pages are destroyed when left).
pragma Singleton
import QtQuick
import org.truesense.app

QtObject {
    id: root

    /// The key picture {keys:[{name,label,cells,x,y,w,h}], width, height}; no keys until loaded.
    property var layout: ({ keys: [], width: 20.5, height: 6.4 })
    property string keysJson: "[]"
    property bool layoutLoaded: false
    /// The names of the looks, built-in first.
    property var looks: ["native"]

    // ---- the view-model --------------------------------------------------------------------

    function _decode(text, verb) {
        var r = JSON.parse(text)
        if (!r.ok) {
            console.warn("lighting " + verb + ": " + r.error)
            return undefined
        }
        return r.result
    }

    /// Calls a verb of the view-model; undefined (and a console warning) when it refuses.
    function call(verb, args) {
        return _decode(Bus.vm("lighting", verb, JSON.stringify(args === undefined ? {} : args)), verb)
    }

    /// Calls a verb of the gallery view-model (which cards a search and a chip leave).
    function gallery(verb, args) {
        return _decode(Bus.vm("gallery", verb, JSON.stringify(args === undefined ? {} : args)), verb)
    }

    /// Like call(), with the key picture's keys added to the arguments without encoding them again.
    function callKeys(verb, args) {
        var t = JSON.stringify(args === undefined ? {} : args)
        return _decode(Bus.vm("lighting", verb, '{"keys":' + keysJson + (t.length > 2 ? "," + t.slice(1) : "}")), verb)
    }

    /// Reads the layout once; `done()` runs when it is available (at once when it already is).
    function loadLayout(done) {
        if (layoutLoaded) {
            if (done) done()
            return
        }
        Api.call("keyboard", "layout", {}, function (answer) {
            var l = call("layout", answer)
            if (l === undefined) return
            layout = l
            keysJson = JSON.stringify(l.keys)
            layoutLoaded = true
            if (done) done()
        })
    }

    /// Reads the look names again.
    function refreshLooks(done) {
        Api.call("look", "list", {}, function (names) {
            looks = Array.isArray(names) ? names : ["native"]
            if (done) done()
        })
    }

    /// How a look is called on screen: the built-in `native` is "Default", the person's own looks
    /// keep the name they typed.
    function lookName(name) {
        //% "Default"
        return name === "native" ? qsTrId("light_look_native") : String(name)
    }

    /// A shipped name ("rainbow-rows") as a title ("Rainbow rows").
    function prettyName(name) {
        var s = String(name).replace(/[-_]+/g, " ")
        return s.charAt(0).toUpperCase() + s.slice(1)
    }

    /// "#rrggbb" for a colour of the view-model ("rrggbb").
    function css(hex) { return "#" + hex }

    // ---- words -----------------------------------------------------------------------------

    function effectName(id) {
        switch (id) {
        //% "Static"
        case "static": return qsTrId("light_fx_static")
        //% "Breathing"
        case "breathing": return qsTrId("light_fx_breathing")
        //% "Wave"
        case "wave": return qsTrId("light_fx_wave")
        //% "Snake"
        case "snake": return qsTrId("light_fx_snake")
        //% "Ripple"
        case "ripple": return qsTrId("light_fx_ripple")
        //% "Neon"
        case "neon": return qsTrId("light_fx_neon")
        //% "Raindrop"
        case "raindrop": return qsTrId("light_fx_raindrop")
        //% "Lightning"
        case "lightning": return qsTrId("light_fx_lightning")
        //% "Spot"
        case "spot": return qsTrId("light_fx_spot")
        //% "Star"
        case "star": return qsTrId("light_fx_star")
        //% "Fireball"
        case "fireball": return qsTrId("light_fx_fireball")
        //% "Snow"
        case "snow": return qsTrId("light_fx_snow")
        //% "Heartbeat"
        case "heartbeat": return qsTrId("light_fx_heartbeat")
        //% "Twinkling"
        case "twinkling": return qsTrId("light_fx_twinkling")
        default: return String(id)
        }
    }

    /// The software animations of the key service.
    function animationName(id) {
        switch (id) {
        //% "Off (still colours)"
        case "off": return qsTrId("light_anim_off")
        //% "Pulse"
        case "pulse": return qsTrId("light_anim_pulse")
        //% "Wave"
        case "wave": return qsTrId("light_anim_wave")
        //% "Colour cycle"
        case "cycle": return qsTrId("light_anim_cycle")
        //% "Twinkle"
        case "twinkle": return qsTrId("light_anim_twinkle")
        //% "Rainbow sweep"
        case "rainbow": return qsTrId("light_anim_rainbow")
        //% "Breathing (your colours)"
        case "breathing": return qsTrId("light_anim_breathing")
        //% "Scanner"
        case "scanner": return qsTrId("light_anim_scanner")
        //% "Comet"
        case "comet": return qsTrId("light_anim_comet")
        //% "Ripples"
        case "ripple": return qsTrId("light_anim_ripple")
        //% "Rain"
        case "rain": return qsTrId("light_anim_rain")
        //% "Matrix rain"
        case "matrix": return qsTrId("light_anim_matrix")
        //% "Fire"
        case "fire": return qsTrId("light_anim_fire")
        //% "Plasma"
        case "plasma": return qsTrId("light_anim_plasma")
        //% "Aurora"
        case "aurora": return qsTrId("light_anim_aurora")
        //% "Lightning"
        case "lightning": return qsTrId("light_anim_lightning")
        //% "Confetti"
        case "confetti": return qsTrId("light_anim_confetti")
        //% "Starlight"
        case "starlight": return qsTrId("light_anim_starlight")
        //% "Mood lamp"
        case "mood": return qsTrId("light_anim_mood")
        //% "Clockwise sweep"
        case "wave-clockwise": return qsTrId("light_anim_wave_cw")
        //% "Counter-clockwise sweep"
        case "wave-counter-clockwise": return qsTrId("light_anim_wave_ccw")
        default: return String(id)
        }
    }

    function modeName(id) {
        switch (id) {
        //% "Eco"
        case "eco": return qsTrId("light_mode_eco")
        //% "Quiet"
        case "quiet": return qsTrId("light_mode_quiet")
        //% "Balanced"
        case "balanced": return qsTrId("light_mode_balanced")
        //% "Performance"
        case "performance": return qsTrId("light_mode_performance")
        //% "Turbo"
        case "turbo": return qsTrId("light_mode_turbo")
        default: return String(id)
        }
    }

    /// One line about a look ("Keyboard Wave, brightness 20, speed 4; rear Static, brightness 80").
    function describeLook(look) {
        if (look === null || look === undefined) return ""
        var s = call("look_summary", look)
        if (s === undefined) return ""
        var parts = []
        if (s.theme) {
            //% "Theme %1"
            parts.push(qsTrId("light_desc_theme").arg(s.theme.id === null ? "" : s.theme.id))
        }
        if (s.keyboard) {
            var k = s.keyboard
            var what = k.kind === "perkey"
                //% "per-key colours"
                ? qsTrId("light_desc_perkey")
                : k.kind === "off"
                    //% "off"
                    ? qsTrId("light_desc_off") : effectName(k.name)
            var text = what
            if (k.brightness !== null) {
                //% "brightness %1"
                text += ", " + qsTrId("light_desc_brightness").arg(k.brightness)
            }
            if (k.speed !== null) {
                //% "speed %1"
                text += ", " + qsTrId("light_desc_speed").arg(k.speed)
            }
            //% "Keyboard: %1"
            parts.push(qsTrId("light_desc_keyboard").arg(text))
        } else {
            //% "Keyboard unchanged"
            parts.push(qsTrId("light_desc_keyboard_same"))
        }
        if (s.rear) {
            //% "Rear bar: %1, brightness %2"
            parts.push(qsTrId("light_desc_rear").arg(effectName(s.rear.mode)).arg(s.rear.brightness === null ? "?" : s.rear.brightness))
        } else {
            //% "Rear bar unchanged"
            parts.push(qsTrId("light_desc_rear_same"))
        }
        return parts.join("; ")
    }
}
