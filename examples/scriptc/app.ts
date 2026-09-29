import {
  add,
  checkedDivide,
  currentStatus,
  echoText,
  EqtsError,
  logOnly,
  makePerson,
  maybeName,
  negate,
  nextByte,
  numbers,
  people,
  reverseBytes,
  scale,
  sumValues,
  words,
} from "./dist/scriptc/index.ts";

console.log("add:", add(20, 22));
console.log("negate:", negate(false));
console.log("scale:", scale(2.5, 4));
console.log("nextByte:", nextByte(255));
logOnly(7);
console.log("echoText:", echoText("hello"));
console.log("sumValues:", sumValues([1, 2, 3, 4]));
console.log("maybeName:", maybeName(true), maybeName(false));

const reversed = reverseBytes(new Uint8Array([1, 2, 3]));
console.log("reverseBytes:", reversed.length, reversed[0]);

const person = makePerson("Ada", 36);
console.log("makePerson:", person.name, person.age);
console.log("currentStatus:", currentStatus());

for await (const value of numbers()) {
  console.log("numbers:", value);
}

for await (const word of words()) {
  console.log("words:", word);
}

for await (const item of people()) {
  console.log("people:", item.name, item.age);
}

try {
  checkedDivide(1, 0);
} catch (error) {
  if (error instanceof EqtsError) {
    console.log("checkedDivide error:", error.code, error.message);
  }
}

console.log("done");
