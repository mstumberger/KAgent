// A crashing view shows a recoverable error card instead of a blank page, and the
// crash (with component stack) is reported to the daemon for the `ui` tool.
import { Component, type ErrorInfo, type ReactNode } from "react";
import { logUi } from "../diag";

interface Props {
  name: string;
  children: ReactNode;
}

export default class ErrorBoundary extends Component<Props, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    logUi("crash", `${this.props.name} view crashed: ${error.name}: ${error.message}`, `${error.stack ?? ""}\ncomponent stack:${info.componentStack ?? ""}`);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="page">
        <div className="card crash">
          <h3>The {this.props.name} view crashed</h3>
          <pre>{this.state.error.message}</pre>
          <p>KAgent can see this error (ask it "what is wrong with the UI?").</p>
          <button className="primary" onClick={() => this.setState({ error: null })}>
            Try again
          </button>{" "}
          <button onClick={() => location.reload()}>Reload page</button>
        </div>
      </div>
    );
  }
}
