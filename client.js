function netpipeIn(host, port, handler) {
  const controller = new AbortController();
  const request = fetch(`http://${host}:${port}`, {
      signal: controller.signal
  });

  (async () => {
      const response = await request;
      const reader = response.body.getReader();
  
      const decoder = new TextDecoder();
      let buffer = "";
      async function read() {
          const { done, value } = await reader.read();
          if (!done) {
              buffer += decoder.decode(value);
              const lines = buffer.split(/\n+/);
              buffer = lines.pop();
              lines.forEach(handler);
              read();
          } else {
              const lines = buffer.trim().split(/\n+/);
              lines.filter(e => e).forEach(handler);
          }
      }
      read();
  })()

  const close = () => controller.abort();
  return { close, request }
}

function netpipeOut(host, port) {
  let controller;
  const stream = new ReadableStream({
      async start(_controller) {
          controller = _controller;
      }
  }).pipeThrough(new TextEncoderStream());
  const request = fetch(`https://${host}:${port}`, {
      method: 'POST',
      headers: { "Content-Type": "text/plain" },
      body: stream,
      duplex: 'half',
      mode: 'cors',
  });
  const write = (text) => controller.enqueue(text);
  const close = () => controller.close();
  return { write, close, request };
}
