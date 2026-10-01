import React from 'react';

interface Props {
  children: React.ReactNode;
}

interface State {
  error: Error | null;
}

// The app has no other error boundary anywhere — without this, any
// uncaught exception during render (a bad localStorage value, a null
// reference, anything) unmounts the whole React tree and leaves a blank
// white window with zero indication of what happened. Reported by a user
// as "el Ragnarok se queda en blanco" — this turns that into a visible,
// recoverable screen instead of a silent failure.
export class ErrorBoundary extends React.Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error('Uncaught render error:', error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div style={{
          position: 'fixed',
          inset: 0,
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 16,
          padding: 32,
          textAlign: 'center',
          background: '#0a0a0f',
          color: '#e5e5e5',
          fontFamily: 'sans-serif',
        }}>
          <div style={{
            width: 56,
            height: 56,
            borderRadius: 16,
            background: 'rgba(239, 68, 68, 0.15)',
            border: '1px solid rgba(239, 68, 68, 0.3)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            fontSize: 26,
          }}>
            ⚠
          </div>
          <div>
            <p style={{ fontSize: 16, fontWeight: 700, margin: 0 }}>Ragnarok Launcher tuvo un error inesperado</p>
            <p style={{ fontSize: 13, color: '#9ca3af', marginTop: 6, maxWidth: 420 }}>
              Algo falló al mostrar la ventana. Reiniciar suele arreglarlo — si sigue pasando, manda este mensaje al soporte.
            </p>
          </div>
          <button
            onClick={() => window.location.reload()}
            style={{
              padding: '10px 24px',
              borderRadius: 12,
              background: '#4f46e5',
              color: 'white',
              fontWeight: 700,
              fontSize: 13,
              border: 'none',
              cursor: 'pointer',
            }}
          >
            Reiniciar
          </button>
          <details style={{ marginTop: 8, fontSize: 11, color: '#6b7280', maxWidth: 500 }}>
            <summary style={{ cursor: 'pointer' }}>Detalles técnicos</summary>
            <pre style={{ whiteSpace: 'pre-wrap', textAlign: 'left', marginTop: 8 }}>
              {this.state.error.message}
              {'\n'}
              {this.state.error.stack}
            </pre>
          </details>
        </div>
      );
    }
    return this.props.children;
  }
}
