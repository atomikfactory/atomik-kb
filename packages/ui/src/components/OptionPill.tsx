import { optionColorOf } from "../property-format";

/**
 * An option as a coloured pill: the name on the tint of its colour (the palette of the editor's
 * highlights, light and dark). A colour this version does not know shows as gray.
 */
export function OptionPill({ name, color }: { name: string; color: string }) {
  return (
    <span className="option-pill" data-color={optionColorOf(color)}>
      {name}
    </span>
  );
}
