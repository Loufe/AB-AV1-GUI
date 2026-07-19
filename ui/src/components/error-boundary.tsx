import { Component, type ReactNode } from "react";

import { Button } from "@/components/ui/button";

interface ErrorBoundaryProps {
  /** Shown in the fallback, e.g. "Statistics view". */
  label: string;
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
}

/**
 * Per-view error boundary (#36 D5): a render crash in one view must not
 * white-screen the app while a conversion is running. Stores and the delta
 * stream live outside React and survive; "Reload view" just remounts the
 * subtree.
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error(`Render error in ${this.props.label}:`, error);
  }

  render() {
    if (this.state.error === null) return this.props.children;
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-8">
        <p className="text-lg">Something went wrong in the {this.props.label}.</p>
        <p className="selectable max-w-xl text-sm text-muted-foreground">
          {this.state.error.message}
        </p>
        <Button variant="outline" onClick={() => this.setState({ error: null })}>
          Reload view
        </Button>
      </div>
    );
  }
}
