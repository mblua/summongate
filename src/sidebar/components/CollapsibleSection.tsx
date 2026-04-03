import { Component, JSX, createSignal, Show } from "solid-js";

interface CollapsibleSectionProps {
  title: string;
  defaultExpanded?: boolean;
  children?: JSX.Element;
}

const CollapsibleSection: Component<CollapsibleSectionProps> = (props) => {
  const [expanded, setExpanded] = createSignal(props.defaultExpanded ?? true);

  return (
    <div class="collapsible-section">
      <div
        class="sidebar-section-header collapsible-section-header"
        onClick={() => setExpanded(!expanded())}
      >
        <span class="collapsible-chevron" classList={{ expanded: expanded() }}>
          &#x25B8;
        </span>
        {props.title}
      </div>
      <Show when={expanded()}>
        {props.children}
      </Show>
    </div>
  );
};

export default CollapsibleSection;
