import type { Config } from 'tailwindcss'

const config: Config = {
  darkMode: ['class'],
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      fontFamily: {
        sans: ['Inter', 'system-ui', '-apple-system', 'sans-serif'],
        mono: ['JetBrains Mono', 'Fira Code', 'ui-monospace', 'monospace'],
      },
      colors: {
        border: 'hsl(var(--border))',
        input: 'hsl(var(--input))',
        ring: 'hsl(var(--ring))',
        background: 'hsl(var(--background))',
        foreground: 'hsl(var(--foreground))',
        muted: {
          DEFAULT: 'hsl(var(--muted))',
          foreground: 'hsl(var(--muted-foreground))',
        },
        card: {
          DEFAULT: 'hsl(var(--card))',
          foreground: 'hsl(var(--card-foreground))',
        },
        primary: {
          DEFAULT: 'hsl(var(--primary))',
          foreground: 'hsl(var(--primary-foreground))',
        },
        secondary: {
          DEFAULT: 'hsl(var(--secondary))',
          foreground: 'hsl(var(--secondary-foreground))',
        },
        destructive: {
          DEFAULT: 'hsl(var(--destructive))',
          foreground: 'hsl(var(--destructive-foreground))',
        },
        accent: {
          DEFAULT: 'hsl(var(--accent))',
          foreground: 'hsl(var(--accent-foreground))',
        },
        popover: {
          DEFAULT: 'hsl(var(--popover))',
          foreground: 'hsl(var(--popover-foreground))',
        },
        success: {
          DEFAULT: 'hsl(var(--success))',
          foreground: 'hsl(var(--success-foreground))',
        },
        warning: {
          DEFAULT: 'hsl(var(--warning))',
          foreground: 'hsl(var(--warning-foreground))',
        },
        'border-subtle': 'hsl(var(--border-subtle))',
        'surface-hover': 'hsl(var(--surface-hover))',
        ember: {
          DEFAULT: 'hsl(var(--primary))',
          surface: 'var(--ember-surface)',
          border: 'var(--ember-border)',
          glow: 'var(--ember-glow)',
          'glow-strong': 'var(--ember-glow-strong)',
          text: 'hsl(var(--ember-text))',
        },
        sidebar: {
          DEFAULT: 'hsl(var(--sidebar))',
          foreground: 'hsl(var(--sidebar-foreground))',
          border: 'hsl(var(--sidebar-border))',
          active: 'hsl(var(--sidebar-active))',
          'active-foreground': 'hsl(var(--sidebar-active-foreground))',
          hover: 'hsl(var(--sidebar-hover))',
        },
      },
      borderRadius: {
        xl: '12px',
        lg: '8px',
        md: '6px',
        sm: '4px',
      },
      boxShadow: {
        xs: 'var(--shadow-xs)',
        soft: 'var(--shadow-soft)',
        card: 'var(--shadow-card)',
        'card-hover': 'var(--shadow-card-hover)',
        float: 'var(--shadow-float)',
        ember: 'var(--shadow-ember)',
      },
      keyframes: {
        'slide-in': {
          from: { opacity: '0', transform: 'translateY(4px)' },
          to: { opacity: '1', transform: 'translateY(0)' },
        },
        'pulse-ember': {
          '0%, 100%': { boxShadow: 'var(--shadow-soft), inset 6px 0 8px -6px rgba(249,115,22,0.3)' },
          '50%': { boxShadow: 'var(--shadow-soft), inset 7px 0 10px -6px rgba(249,115,22,0.6)' },
        },
      },
      animation: {
        'slide-in': 'slide-in 150ms ease-out',
        'pulse-ember': 'pulse-ember 2.2s ease-in-out infinite',
      },
      fontSize: {
        micro: ['10px', { lineHeight: '1.2' }],
        ui: ['13px', { lineHeight: '1.45' }],
        page: ['22px', { lineHeight: '1.3' }],
      },
    },
  },
  plugins: [require('@tailwindcss/typography')],
}

export default config
