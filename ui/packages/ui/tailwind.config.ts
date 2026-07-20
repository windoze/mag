import type { Config } from "tailwindcss";

export default {
  content: ["./src/**/*.{ts,tsx}", "./.storybook/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        border: "hsl(var(--border))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))"
        }
      },
      borderRadius: {
        lg: "var(--radius)"
      }
    }
  },
  plugins: []
} satisfies Config;
