/* Android phone or tablet actions for the shared screenshot stream viewer. */

import { mountStream } from "./viewer-stream.js?v=0.5.2";

export function mountAndroid({ host, screen, ui }) {
  return mountStream({
    host,
    screen,
    ui,
    wsPath: `/ws/android/${host.id}`,
    deviceHint: `The server could not reach Android device ${host.target}. Run \`adb devices\`, accept the debugging prompt on the device, and make sure the serial is online.`,
    buildActions: ({ send, saveScreenshot }) => [
      {
        id: "home",
        label: "Home",
        icon: "i-home",
        run: () => send({ type: "button", name: "home" }),
      },
      {
        id: "back",
        label: "Back",
        icon: "i-arrow-left",        run: () => send({ type: "button", name: "back" }),
      },
      {
        id: "recents",
        label: "Recents",
        icon: "i-recents",
        run: () => send({ type: "button", name: "recents" }),
      },
      {
        id: "power",
        label: "Power",
        icon: "i-lock",
        run: () => send({ type: "button", name: "power" }),
      },
      {
        id: "type",
        label: "Type",
        icon: "i-keyboard",
        run: () => {
          const text = window.prompt("Send text to the Android device");
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
        value: "5",
        options: [
          { value: "3", label: "3 fps" },
          { value: "5", label: "5 fps" },
          { value: "8", label: "8 fps" },
          { value: "12", label: "12 fps" },
        ],
        onChange: (value) => send({ type: "set_fps", value: Number(value) }),
      },
    ],
  });
}
