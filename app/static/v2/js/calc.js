// Sums typed in the amount field: "120+45,5" or "3*250" are saved as their result.

/** Value of an expression with + - * / and parentheses, or NaN when it is not one. */
export function evalAmount(text) {
  const src = String(text || "")
    .replace(/\s+/g, "")
    .replace(/,/g, ".")
    .replace(/[×xх]/gi, "*")
    .replace(/[÷:]/g, "/")
    .replace(/[−–]/g, "-");
  if (!src) return NaN;
  // An operator typed last (still writing "100+") is ignored.
  const trimmed = src.replace(/[+\-*/.(]+$/, "");
  if (trimmed !== src) return evalAmount(trimmed);
  let i = 0;
  const peek = () => src[i];
  const number = () => {
    const m = /^\d*\.?\d+|^\d+\.?/.exec(src.slice(i));
    if (!m) throw new Error("number expected");
    i += m[0].length;
    return parseFloat(m[0]);
  };
  const factor = () => {
    if (peek() === "-") return i++, -factor();
    if (peek() === "+") return i++, factor();
    if (peek() === "(") {
      i++;
      const v = sum();
      if (peek() === ")") i++;
      return v;
    }
    return number();
  };
  const product = () => {
    let v = factor();
    while (peek() === "*" || peek() === "/") v = src[i++] === "*" ? v * factor() : v / factor();
    return v;
  };
  const sum = () => {
    let v = product();
    while (peek() === "+" || peek() === "-") v = src[i++] === "+" ? v + product() : v - product();
    return v;
  };
  try {
    const v = sum();
    if (i !== src.length || !Number.isFinite(v)) return NaN;
    return Math.round(v * 100) / 100;
  } catch {
    return NaN;
  }
}

export function isExpression(text) {
  return /\d\s*[-+*/×÷−]/.test(String(text || "").replace(/^\s*-/, ""));
}
