{
  startOnLoad: false,
  securityLevel: 'loose',
  // A single high-contrast light theme, rendered on a white card in both page
  // themes — so diagram text is always dark-on-light and legible.
  theme: 'base',
  themeVariables: {
    fontFamily: 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif',
    fontSize: '15px',
    primaryColor: '#fff7ed',
    primaryBorderColor: '#f59e0b',
    primaryTextColor: '#0f172a',
    secondaryColor: '#eff6ff',
    secondaryBorderColor: '#2563eb',
    secondaryTextColor: '#0f172a',
    tertiaryColor: '#f8fafc',
    tertiaryBorderColor: '#cbd5e1',
    tertiaryTextColor: '#0f172a',
    lineColor: '#475569',
    textColor: '#0f172a',
    noteBkgColor: '#fef3c7',
    noteBorderColor: '#f59e0b',
    noteTextColor: '#0f172a',
    clusterBkg: '#f8fafc',
    clusterBorder: '#cbd5e1'
  },
  // useMaxWidth:false renders every diagram at its natural size — text stays
  // full-size and readable, and the wrapper scrolls for wide/large graphs
  // instead of shrinking everything until nobody can read it.
  flowchart: {
    useMaxWidth: false,
    htmlLabels: true,
    curve: 'basis',
    nodeSpacing: 48,
    rankSpacing: 56,
    padding: 14
  },
  sequence: {
    useMaxWidth: false,
    actorMargin: 64,
    boxMargin: 12,
    messageFontSize: 14,
    noteFontSize: 13
  },
  gantt: { useMaxWidth: false, fontSize: 13 },
  er: { useMaxWidth: false },
  journey: { useMaxWidth: false },
  state: { useMaxWidth: false },
  class: { useMaxWidth: false }
}
