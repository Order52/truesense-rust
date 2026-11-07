// A small server-sent-events parser. The page reads /api/stream with fetch(), because the
// browser's built-in event-source object cannot send the Authorization header, so it parses
// the event stream itself.

/**
 * @param {(ev: {event: string, data: string}) => void} onEvent
 * @returns {{push: (chunk: string) => void}}
 */
export function createSSEParser(onEvent) {
  let buffer = "";
  let event = "message";
  let data = [];
  const dispatch = () => {
    if (data.length) onEvent({ event, data: data.join("\n") });
    event = "message";
    data = [];
  };
  const line = (l) => {
    if (l === "") return dispatch();
    if (l.startsWith(":")) return; // a comment (the server's keep-alive ping)
    const i = l.indexOf(":");
    const field = i < 0 ? l : l.slice(0, i);
    let value = i < 0 ? "" : l.slice(i + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") event = value;
    else if (field === "data") data.push(value);
  };
  return {
    push(chunk) {
      buffer += chunk;
      let m;
      while ((m = /\r\n|\n|\r/.exec(buffer))) {
        // a lone \r at the very end may be the first half of \r\n: wait for more
        if (m[0] === "\r" && m.index === buffer.length - 1) break;
        line(buffer.slice(0, m.index));
        buffer = buffer.slice(m.index + m[0].length);
      }
    },
  };
}
