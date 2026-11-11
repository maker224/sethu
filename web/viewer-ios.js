/* iPhone / iPad actions for the shared screenshot stream viewer. */

import { mountStream } from "./viewer-stream.js?v=0.9.0";

export function mountIos({ host, screen, ui }) {
  return mountStream({
    host,
    screen,
    ui,
    wsPath: `/ws/ios/${host.id}`,
    deviceHint: `The server could not reach WebDriverAgent at ${host.target}. Check that the iPhone or iPad is connected, WDA is running, and the port forward is up.`,
    buildActions: ({ send, saveScreenshot }) => [
      {
        id: "home",
        label: "Home",
        icon: "i-home",
        run: () => send({ type: "button", name: "home" }),
      },
      {
        id: "lock",
        label: "Lock",
        icon: "i-lock",
        run: () => send({ type: "button", name: "lock" }),
      },
      {
        id: "type",
        label: "Type",
        icon: "i-keyboard",
        run: () => {
          const text = window.prompt("Send text to the iPhone or iPad");
          if (text) send({ type: "keys", text });
        },
      },
      {
        id: "screenshot",
        label: "Save image",
        icon: "i-camera",
        run: saveScreenshot,
      },
      {
        type: "select",
        label: "Frame rate",
        value: "10",
        options: [
          { value: "4", label: "4 fps" },
          { value: "8", label: "8 fps" },
          { value: "10", label: "10 fps" },
          { value: "15", label: "15 fps" },
        ],
        onChange: (value) => send({ type: "set_fps", value: Number(value) }),
      },
    ],
  });
}
