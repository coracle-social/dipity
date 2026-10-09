import type {CapacitorConfig} from "@capacitor/cli"

const config: CapacitorConfig = {
  appId: "social.coracle.dipity",
  appName: "Dipity",
  webDir: "dist",
  // Capacitor disables WKWebView's pinch by default; page zoom is the accessibility affordance index.html keeps.
  ios: {zoomEnabled: true},
}

export default config
