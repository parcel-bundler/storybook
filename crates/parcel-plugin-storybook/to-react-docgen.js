"use strict";

function toReactDocgen(component) {
  if (!component) {
    return null;
  }
  if (component.displayName && component.props) {
    return component;
  }

  let props = {};
  for (let [name, property] of Object.entries(getProperties(component.props))) {
    if (property.type !== "property") {
      continue;
    }
    props[name] = {
      name,
      required: !property.optional,
      type: toPropType(property.value, !property.optional),
      description: property.description || "",
      defaultValue:
        property.default == null
          ? null
          : { value: parseDefault(property.default) },
    };
  }

  return {
    displayName: component.name || nameFromId(component.id) || "Component",
    filePath: fileFromId(component.id),
    description: component.description || "",
    props,
    methods: [],
  };
}

function getProperties(node) {
  if (!node) {
    return {};
  }
  switch (node.type) {
    case "interface":
    case "object":
      return node.properties || {};
    case "alias":
      return getProperties(node.value);
    case "intersection":
      return Object.assign({}, ...node.types.map(getProperties));
    default:
      return {};
  }
}

function toPropType(node, required) {
  if (node?.type === "union") {
    let elements = node.elements.filter(
      (element) => required || element.type !== "undefined"
    );
    if (elements.every(isLiteralType)) {
      return {
        name: "enum",
        raw: elements.map(printType).join(" | "),
        value: elements.map((element) => ({
          value: printType(element),
          ...(element.description ? { description: element.description } : {}),
        })),
      };
    }
    return { name: elements.map(printType).join(" | ") };
  }
  return { name: printType(node) };
}

function isLiteralType(node) {
  return (
    node?.type === "undefined" ||
    ((node?.type === "string" || node?.type === "number") && node.value != null)
  );
}

function parseDefault(defaultValue) {
  if (typeof defaultValue === "string") {
    defaultValue = defaultValue.replace(/^['"](.+)['"].*$/, '"$1"');
    try {
      defaultValue = JSON.parse(defaultValue);
    } catch {
      // ignore
    }
  }
  return defaultValue;
}

function printType(node) {
  if (!node) {
    return "unknown";
  }
  switch (node.type) {
    case "any":
    case "null":
    case "undefined":
    case "void":
    case "unknown":
    case "never":
    case "this":
    case "symbol":
      return node.type;
    case "identifier":
    case "typeParameter":
      return node.name;
    case "string":
      return node.value == null ? "string" : JSON.stringify(node.value);
    case "number":
      return node.value == null ? "number" : String(node.value);
    case "boolean":
      return node.value == null ? "boolean" : String(node.value);
    case "union":
      return node.elements.map(printType).join(" | ");
    case "intersection":
      return node.types.map(printType).join(" & ");
    case "application":
      return `${printType(node.base)}<${node.typeParameters
        .map(printType)
        .join(", ")}>`;
    case "typeOperator":
      return `${node.operator} ${printType(node.value)}`;
    case "function":
      return `(${node.parameters
        .map(printParameter)
        .join(", ")}) => ${printType(node.return)}`;
    case "parameter":
      return printParameter(node);
    case "property":
      return printType(node.value);
    case "method":
      return printType(node.value);
    case "alias":
    case "interface":
      return node.name;
    case "object":
      return printObject(node.properties);
    case "array": {
      let element = printType(node.elementType);
      return needsParens(node.elementType) ? `(${element})[]` : `${element}[]`;
    }
    case "tuple":
      return `[${node.elements.map(printType).join(", ")}]`;
    case "template":
      return `\`${node.elements.map(printType).join("")}\``;
    case "component":
      return node.name || "React.ComponentType";
    case "conditional":
      return `${printType(node.checkType)} extends ${printType(
        node.extendsType
      )} ? ${printType(node.trueType)} : ${printType(node.falseType)}`;
    case "indexedAccess":
      return `${printType(node.objectType)}[${printType(node.indexType)}]`;
    case "mapped":
      return `{[${printParameter(node.typeParameter)}]: ${printType(
        node.typeAnnotation
      )}}`;
    case "link":
      return nameFromId(node.id) || "unknown";
    case "reference":
      return node.local || node.imported || node.specifier || "unknown";
    default:
      return "unknown";
  }
}

function printParameter(node) {
  if (!node) {
    return "unknown";
  }
  return `${node.rest ? "..." : ""}${node.name || "arg"}${
    node.optional ? "?" : ""
  }: ${printType(node.value || node.constraint)}`;
}

function printObject(properties) {
  if (!properties) {
    return "object";
  }
  let values = Object.entries(properties).map(
    ([name, property]) =>
      `${name}${property.optional ? "?" : ""}: ${printType(property.value)}`
  );
  return `{ ${values.join("; ")} }`;
}

function needsParens(node) {
  return node?.type === "union" || node?.type === "intersection";
}

function nameFromId(id) {
  return id?.slice(id.lastIndexOf(":") + 1) || "";
}

function fileFromId(id) {
  let index = id?.lastIndexOf(":") ?? -1;
  return index < 0 ? "" : id.slice(0, index);
}

module.exports.toReactDocgen = toReactDocgen;
