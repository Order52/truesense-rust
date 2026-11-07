import QtQuick
import QtTest
import org.truesense.app

TestCase {
    name: "Fmt"

    // The table shared with ts-ui-model's Rust tests (crates/ts-ui-model/tests/format-vectors.json).
    function test_matches_the_rust_vectors() {
        var xhr = new XMLHttpRequest()
        xhr.open("GET", Qt.resolvedUrl("../../../ts-ui-model/tests/format-vectors.json"), false)
        xhr.send()
        verify(xhr.responseText.length > 100, "vectors file not readable (set QML_XHR_ALLOW_FILE_READ=1)")
        var vectors = JSON.parse(xhr.responseText)
        verify(vectors.length > 20)
        for (var i = 0; i < vectors.length; ++i) {
            var v = vectors[i]
            var got = Fmt[v.fn].apply(Fmt, v.args)
            compare(got, v.expect, v.fn + "(" + JSON.stringify(v.args) + ")")
        }
    }

    function test_non_numbers_are_dashes() {
        compare(Fmt.temp(undefined), "–")
        compare(Fmt.temp("54"), "–")
        compare(Fmt.rpm(NaN), "–")
        compare(Fmt.temp(Infinity), "–")
    }
}
