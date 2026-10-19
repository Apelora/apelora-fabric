import type { ConfigValue, Property } from "./schema";

const SECRET_REF = /^[a-z0-9][a-z0-9._/-]{0,63}$/;

/** The value an instance actually uses: its override, else the class default. */
export function effectiveValue(prop: Property, config: Readonly<Record<string, ConfigValue>> | undefined): ConfigValue | undefined {
  const v = config?.[prop.id];
  return v === undefined ? prop.default : v;
}

/** Returns a human-readable problem with `value` for `prop`, or null when it is acceptable. */
export function validateProperty(prop: Property, value: ConfigValue | undefined): { message: string; hint: string } | null {
  const empty = value === undefined || value === "";
  if (empty) return prop.required ? { message: `${prop.label} is required.`, hint: `Enter a value for ${prop.label}.` } : null;
  switch (prop.type) {
    case "int": {
      if (typeof value !== "number" || !Number.isInteger(value)) return { message: `${prop.label} must be a whole number.`, hint: "Use digits only." };
      if (prop.min !== undefined && value < prop.min) return { message: `${prop.label} must be at least ${prop.min}.`, hint: `Minimum is ${prop.min}.` };
      if (prop.max !== undefined && value > prop.max) return { message: `${prop.label} must be at most ${prop.max}.`, hint: `Maximum is ${prop.max}.` };
      return null;
    }
    case "enum":
      return prop.options && !prop.options.includes(String(value)) ? { message: `${prop.label}: "${String(value)}" is not an allowed option.`, hint: `Choose one of: ${prop.options.join(", ")}.` } : null;
    case "secret":
      return typeof value === "string" && SECRET_REF.test(value) ? null : { message: `${prop.label} must be a secret reference name.`, hint: "Use a short name such as db-password (lowercase letters, digits, . _ / -). Never paste the secret itself." };
    case "bool":
      return typeof value === "boolean" ? null : { message: `${prop.label} must be on or off.`, hint: "Use the checkbox." };
    case "string":
      return typeof value === "string" && value.length <= 256 ? null : { message: `${prop.label} is too long.`, hint: "Use at most 256 characters." };
  }
}
