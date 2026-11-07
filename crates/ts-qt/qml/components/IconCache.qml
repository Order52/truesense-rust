// The text of every icon file read so far; Icon asks for names, the file is read once.
pragma Singleton
import QtQuick

QtObject {
    property var texts: ({})

    function text(name, url) {
        var cached = texts[name]
        if (cached !== undefined) return cached
        var out = ""
        try {
            var xhr = new XMLHttpRequest()
            xhr.open("GET", url, false)
            xhr.send()
            if (xhr.status === 200 || xhr.status === 0) out = xhr.responseText
        } catch (e) {
            out = ""
        }
        texts[name] = out
        return out
    }
}
