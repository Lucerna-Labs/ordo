import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        space: "#020617",
        liquid: {
          teal: "#0d9488",
          blue: "#3b82f6",
          amber: "#78350f",
          crimson: "#991b1b",
        },
      },
      boxShadow: {
        aura: "0 0 80px rgba(13, 148, 136, 0.22)",
        alert: "0 0 60px rgba(153, 27, 27, 0.3)",
      },
      backdropBlur: {
        "liquid-xl": "60px",
      },
      transitionTimingFunction: {
        liquid: "cubic-bezier(0.22, 1, 0.36, 1)",
      },
    },
  },
  plugins: [],
} satisfies Config;
