// The functions of section 3 and the operators of 3.2, case by case (the corpus covers the contract; this covers the
// rest of the rules of the twin).

import { describe, expect, it } from "vitest";

import { isFormulaError } from "./index";
import { makeFixture, plain } from "./host.fixture";
import type { PageSpec, PropertySpec } from "./host.fixture";

const fx = makeFixture();

function value(expression: string, page = "p1"): unknown {
  return plain(fx.run(expression, page));
}

function table(title: string, rows: readonly (readonly [string, unknown])[]): void {
  describe(title, () => {
    for (const [expression, expected] of rows) {
      it(expression, () => {
        expect(value(expression)).toEqual(expected);
      });
    }
  });
}

table("operators", [
  ['"a" + 1', "a1"],
  ['1 + "a"', "1a"],
  ['"a" + true', "atrue"],
  ['"x" + prop("Tags")', "xred, blue"],
  ['"d: " + prop("Due")', "d: October 25, 2026"],
  ['"n=" + at([1], 5)', "n="],
  ["1 + 2", 3],
  ["10 / 4", 2.5],
  ["5.5 % 2", 1.5],
  ["2 ^ 10", 1024],
  ["0 / 0", null],
  ["(-8) ^ (1 / 3)", null],
  ["- -3", 3],
  ["--3", 3],
  ["1e308 * 10", null],
  ["at([1], 5) + 1", null],
  ["false < true", true],
  ['"a" < "B"', false],
  ['"😀" > "ｱ"', true],
  ['"abc" < "abd"', true],
  ['"" < "a"', true],
  ["parseDate(\"2026-01-01\") < parseDate(\"2026-01-01T00:01\")", true],
  ["parseDate(\"2026-01-01\") == parseDate(\"2026-01-01T00:00\")", true],
  ["[1, 2] == [1, 2]", true],
  ["[1, 2] == [2, 1]", false],
  ["[] == []", true],
  ["[1] == []", false],
  ['"a" == "a"', true],
  ["at([1], 5) == at([1], 5)", true],
  ["at([1], 5) == 1", false],
  ["at([1], 5) != 1", true],
  ["at([1], 5) < 1", false],
  ["at([1], 5) >= 1", false],
  ["true && false || true", true],
  ["true and false or true", true],
  ["not not true", true],
  ["!false", true],
  ["true.not()", false],
  ["true.and(false)", false],
  ["false.or(true)", true],
  ["false ? 1 : false ? 2 : 3", 3],
  ["true ? false ? 1 : 2 : 3", 2],
]);

table("numbers", [
  ["abs(-3)", 3],
  ["ceil(1.2)", 2],
  ["ceil(-1.5)", -1],
  ["floor(-1.5)", -2],
  ["ceil(-0.5)", 0],
  ["sqrt(16)", 4],
  ["sqrt(-1)", null],
  ["cbrt(27)", 3],
  ["exp(0)", 1],
  ["ln(1)", 0],
  ["ln(0)", null],
  ["sign(-5)", -1],
  ["sign(0)", 0],
  ["sign(7)", 1],
  ["pi()", Math.PI],
  ["e()", Math.E],
  ["add(1, 2)", 3],
  ["subtract(1, 2)", -1],
  ["multiply(3, 4)", 12],
  ["divide(1, 4)", 0.25],
  ["divide(1, 0)", null],
  ["mod(7, 4)", 3],
  ["mod(7, 0)", null],
  ["pow(2, 3)", 8],
  ["round(2.5)", 3],
  ["round(-0.4)", 0],
  ["round(1234.5678, 2)", 1234.57],
  ["round(1234, -2)", 1200],
  ["round(0.5)", 1],
  ["round(1.5, 0.9)", 2],
  ["round(2.55, 1)", 2.6],
  ["round(1.23456789, 100)", 1.23456789],
  ["toNumber(\"1e3\")", 1000],
  ['toNumber("+5")', 5],
  ['toNumber("-.5")', -0.5],
  ['toNumber("5.")', 5],
  ['toNumber("1,5")', null],
  ['toNumber("")', null],
  ['toNumber("Infinity")', null],
  ['toNumber("1e999")', null],
  ['toNumber("0x10")', null],
  ["toNumber(\" 5 \")", 5],
  ["toNumber(true)", 1],
  ["toNumber(false)", 0],
  ["toNumber(7)", 7],
  ["min(3, 1, 2)", 1],
  ["max([1, 5], 3)", 5],
  ["min([])", null],
  ["max([], [])", null],
  ["sum(1, [2, 3])", 6],
  ["mean(1, 2)", 1.5],
  ["median(1, 2, 3)", 2],
  ["median(4, 1)", 2.5],
  ['prop("Tasks").map(current.prop("Hours")).mean()', 3.75],
  ['prop("Tasks").map(current.prop("Hours")).median()', 3.75],
  ['prop("Tasks").map(current.prop("Hours")).max()', 5],
  ['prop("Tasks").map(current.prop("Hours")).min()', 2.5],
  ["max(parseDate(\"2026-01-05\"), parseDate(\"2025-12-31\")).month()", 1],
]);

table("text", [
  ['length("")', 0],
  ['length("héllo")', 5],
  ["length([1, 2, 3])", 3],
  ["length([])", 0],
  ['substring("hello", 1)', "ello"],
  ['substring("hello", -2, 2)', "he"],
  ['substring("hello", 3, 1)', null],
  ['substring("hello", 2, 100)', "llo"],
  ['substring("😀ab", 1, 2)', "a"],
  ['substring("hello", 1.9, 3.9)', "el"],
  ['contains("abc", "")', true],
  ['contains("abc", "bc")', true],
  ['contains("abc", "B")', false],
  ['test("abc", "B")', false],
  ['test("abc", "(")', null],
  ['test("a.c", "a\\\\.c")', true],
  ['test("abc", "a\\\\.c")', false],
  ['test("a-c", "a\\\\-c")', true],
  ['test("a#c", "a\\\\#c")', true],
  ['test("a\\nb", "a.b")', false],
  ['test("a\rb", "a.b")', false],
  ['test("a b", "a.b")', false],
  ['test("a b", "a.b")', false],
  ['test("a\tb", "a.b")', true],
  ['test("😀", "^.$")', true],
  ['test("a", "(?=a)")', null],
  ['test("a", "(?!b)")', null],
  ['test("a", "(?<=a)")', null],
  ['test("a", "(?<n>a)")', null],
  ['test("a", "(?i)A")', null],
  ['test("aa", "(a)\\\\1")', null],
  ['test("a", "\\\\x61")', null],
  ['test("a", "\\\\u0061")', null],
  ['test("a", "\\\\p{L}")', null],
  ['test("a", "\\\\0")', null],
  ['test("a", "a\\\\")', null],
  ['test("a", "[a")', null],
  ['test("a", "a)")', null],
  ['test("a", "*")', null],
  ['test("a", "a{")', null],
  ['test("a", "[\\\\B]")', null],
  ['test("ab", "(?:a)b")', true],
  ['test("a]", "a]")', true],
  ['test("a}", "a}")', true],
  ['test("aaa", "^a{3}$")', true],
  ['test("aaa", "^a{2,}$")', true],
  ['test("aaa", "^a{1,2}$")', false],
  ['test("a b", "a\\\\sb")', true],
  ['test("a\u0085b", "a\\\\sb")', true],
  ['test("a　b", "a\\\\sb")', true],
  ['test("a﻿b", "a\\\\sb")', false],
  ['test("a﻿b", "a\\\\Sb")', true],
  ['test("a b", "a[\\\\s]b")', true],
  ['test("ab", "[]")', false],
  ['test("ab", "[^]")', true],
  ['test("a/b", "a\\\\/b")', true],
  ['test("a$b", "a\\\\$b")', true],
  ['test("a\\\\b", "a\\\\\\\\b")', true],
  ['test("é", "^[^\\\\d]$")', true],
  ['test("ab", "^(a|b)+$")', true],
  ['test("A1", "^\\\\w\\\\d$")', true],
  ['match("a1b22c333", "[0-9]+")', ["1", "22", "333"]],
  ['match("abc", "[0-9]+")', null],
  ['match("abc", "[0-9]+").length()', 0],
  ['match("baab", "a*").length()', 4],
  ['match("abc", "").length()', 4],
  ['match("a1", "(")', null],
  ['replace("aaa", "a", "b")', "baa"],
  ['replaceAll("aaa", "a", "b")', "bbb"],
  ['replace("abc", "(b)", "[$1$1]")', "a[bb]c"],
  ['replace("abc", "(b)", "$$")', "a$c"],
  ['replace("abc", "b", "$0$0")', "abbc"],
  ['replace("abc", "b", "$2")', "ac"],
  ['replace("abc", "b", "$x")', "a$xc"],
  ['replace("abc", "b", "$")', "a$c"],
  ['replace("abc", "(z)?b", "[$1]")', "a[]c"],
  ['replace("abc", "z", "y")', "abc"],
  ['replaceAll("abc", "", "-")', "-a-b-c-"],
  ['replaceAll("baab", "a*", "X")', "XbXXbX"],
  ['replaceAll("a.b", "\\\\.", "$$")', "a$b"],
  ['replaceAll("😀😀", "", "-")', "-😀-😀-"],
  ['replace("abc", "(", "x")', null],
  ['split("a,b", ",")', ["a", "b"]],
  ['split("abc", "")', ["a", "b", "c"]],
  ['split("😀a", "")', ["😀", "a"]],
  ['split("", ",").length()', 1],
  ['split("", "").length()', 0],
  ['split("a--b", "--")', ["a", "b"]],
  ['join(["a", "b"], "-")', "a-b"],
  ["join([1, 2], \"+\")", "1+2"],
  ['join([], ",")', null],
  ['join(prop("Tasks"), " & ")', "Task A & Task B"],
  ['lower("ÀB")', "àb"],
  ['upper("straße")', "STRASSE"],
  ['lower("İ").length()', 2],
  ['lower("ΑΣ")', "ας"],
  ['trim("  a b \\n")', "a b"],
  ['trim(" x ")', "x"],
  ['trim("\u0085x ")', "x"],
  ['trim("﻿x")', "﻿x"],
  ['repeat("ab", 3)', "ababab"],
  ['repeat("a", -1)', null],
  ['repeat("a", 2.9)', "aa"],
  ['repeat("ab", 6000)', null],
  ['repeat("a", 10000).length()', 10000],
  ['repeat("a", 99999).length()', 10000],
  ['repeat("", 5)', null],
  ['padStart("7", 3, "0")', "007"],
  ['padStart("abc", 2, "0")', "abc"],
  ['padStart("7", 6, "ab")', "ababa7"],
  ['padEnd("7", 4, "ab")', "7aba"],
  ['padStart("x", 3, "")', "x"],
  ['padStart("x", 20000, "0")', null],
  ['padStart("😀", 3, "ab")', "ab😀"],
  ["format(1.5)", "1.5"],
  ["format(true)", "true"],
  ["format([1, 2])", "1, 2"],
  ["format(0.000001)", "0.000001"],
  ["format(0.0000001)", "1e-7"],
  ["format(123456789012345680000)", "123456789012345680000"],
  ["format(1e21)", "1e+21"],
  ["format(-1.5e-7)", "-1.5e-7"],
  ['format(prop("Due"))', "October 25, 2026"],
  ["format(now())", "October 18, 2026 9:30 AM"],
  ["format(today())", "October 18, 2026"],
  ['format(prop("Owner"))', "Ada"],
  ['format(prop("Tags"))', "red, blue"],
  ['format(prop("Tasks").first())', "Task A"],
  ["format([\"a\", at([\"b\"], 3), \"c\"])", "a, , c"],
]);

table("dates", [
  ['year(parseDate("2026-10-18"))', 2026],
  ['month(parseDate("2026-10-18"))', 10],
  ['date(parseDate("2026-10-18"))', 18],
  ['day(parseDate("2026-10-18"))', 7],
  ['day(parseDate("2026-10-19"))', 1],
  ['day(parseDate("1970-01-01"))', 4],
  ['hour(parseDate("2026-10-18T14:05"))', 14],
  ['minute(parseDate("2026-10-18T14:05"))', 5],
  ['hour(parseDate("2026-10-18"))', 0],
  ['week(parseDate("2020-12-31"))', 53],
  ['week(parseDate("2021-01-03"))', 53],
  ['week(parseDate("2021-01-04"))', 1],
  ['week(parseDate("2024-12-30"))', 1],
  ['week(parseDate("2026-10-18"))', 42],
  ['formatDate(dateAdd(parseDate("2024-02-29"), 1, "years"), "YYYY-MM-DD")', "2025-02-28"],
  ['formatDate(dateAdd(parseDate("2026-10-18"), 1, "quarters"), "YYYY-MM-DD")', "2027-01-18"],
  ['formatDate(dateAdd(parseDate("2026-10-18"), -1, "weeks"), "YYYY-MM-DD")', "2026-10-11"],
  ['formatDate(dateAdd(parseDate("2026-10-18"), 36, "hours"), "YYYY-MM-DD HH:mm")', "2026-10-19 12:00"],
  ['formatDate(dateAdd(parseDate("2026-10-18T10:00"), 90, "minutes"), "HH:mm")', "11:30"],
  ['formatDate(dateAdd(parseDate("2026-10-18"), 1.9, "days"), "YYYY-MM-DD")', "2026-10-19"],
  ['formatDate(dateAdd(parseDate("2026-10-18"), -1.9, "days"), "YYYY-MM-DD")', "2026-10-17"],
  ['formatDate(dateAdd(parseDate("2026-01-31T10:00"), 1, "months"), "YYYY-MM-DD HH:mm")', "2026-02-28 10:00"],
  ['formatDate(dateSubtract(parseDate("2026-03-31"), 1, "months"), "YYYY-MM-DD")', "2026-02-28"],
  ['formatDate(dateSubtract(parseDate("2026-01-15"), 2, "months"), "YYYY-MM-DD")', "2025-11-15"],
  ['formatDate(dateAdd(parseDate("2026-01-15"), -13, "months"), "YYYY-MM-DD")', "2024-12-15"],
  ['dateAdd(parseDate("2026-10-18"), 1, "day")', null],
  ['dateAdd(parseDate("2026-10-18"), 1, "")', null],
  ['dateAdd(parseDate("9999-12-31"), 1, "days")', null],
  ['dateAdd(parseDate("0001-01-01"), -1, "days")', null],
  ['dateAdd(parseDate("2026-01-01"), 1e15, "days")', null],
  ['dateAdd(parseDate("2026-01-01"), 1e300, "years")', null],
  ['dateAdd(parseDate("2026-01-01"), 8000, "years")', null],
  ['formatDate(dateAdd(parseDate("2026-01-01"), 7000, "years"), "YYYY")', "9026"],
  ['dateBetween(parseDate("2026-10-18T10:00"), parseDate("2026-10-17T11:00"), "days")', 0],
  ['dateBetween(parseDate("2026-10-17T11:00"), parseDate("2026-10-18T10:00"), "days")', 0],
  ['dateBetween(parseDate("2026-12-25"), parseDate("2026-10-18"), "weeks")', 9],
  ['dateBetween(parseDate("2026-10-18"), parseDate("2025-10-19"), "years")', 0],
  ['dateBetween(parseDate("2026-10-19"), parseDate("2025-10-19"), "years")', 1],
  ['dateBetween(parseDate("2026-10-18"), parseDate("2026-01-18"), "quarters")', 3],
  ['dateBetween(parseDate("2026-01-18"), parseDate("2026-10-18"), "quarters")', -3],
  ['dateBetween(parseDate("2026-10-18"), parseDate("2026-10-18"), "days")', 0],
  ['dateBetween(parseDate("2026-02-28"), parseDate("2026-01-31"), "months")', 0],
  ['dateBetween(parseDate("2026-03-01"), parseDate("2026-01-31"), "months")', 1],
  ['dateBetween(parseDate("2026-10-18T09:00"), parseDate("2026-09-18T10:00"), "months")', 0],
  ['dateBetween(parseDate("2026-10-18T10:00"), parseDate("2026-09-18T10:00"), "months")', 1],
  ['dateBetween(parseDate("2026-10-18T10:30"), parseDate("2026-10-18T10:00"), "minutes")', 30],
  ['dateBetween(parseDate("2026-10-18"), parseDate("2026-10-18T10:30"), "minutes")', -630],
  ['dateBetween(parseDate("2026-10-18"), parseDate("2026-10-17"), "decades")', null],
  [
    'formatDate(parseDate("2026-01-05T15:07:09"), "YYYY YY Q MMMM MMM MM M DD D Do dddd ddd d HH H hh h mm m A a ss s")',
    "2026 26 1 January Jan 01 1 05 5 5th Monday Mon 1 15 15 03 3 07 7 PM pm 09 9",
  ],
  ['formatDate(parseDate("2026-01-01"), "[YYYY] YYYY")', "YYYY 2026"],
  ['formatDate(parseDate("2026-01-01"), "[x YYYY")', "[x 2026"],
  ['formatDate(parseDate("2026-01-01"), "[]YYYY")', "2026"],
  ['formatDate(parseDate("2026-01-01"), "YYYY/MM/DD @ HH:mm")', "2026/01/01 @ 00:00"],
  ['formatDate(parseDate("2026-01-01"), "")', null],
  ['formatDate(parseDate("2026-01-01"), "YYY")', "26Y"],
  ['formatDate(parseDate("2026-01-01"), "DDo")', "01o"],
  ['formatDate(parseDate("2026-01-01T00:30"), "hh:mm A")', "12:30 AM"],
  ['formatDate(parseDate("2026-01-01T12:30"), "h:mm a")', "12:30 pm"],
  ['formatDate(parseDate("2026-01-01T23:59"), "h:mm A")', "11:59 PM"],
  ['formatDate(parseDate("2026-01-01"), "HH:mm")', "00:00"],
  ['formatDate(parseDate("2026-01-01"), "d dddd")', "4 Thursday"],
  ['formatDate(parseDate("2026-01-04"), "d ddd")', "0 Sun"],
  ['formatDate(parseDate("2026-02-01"), "Do")', "1st"],
  ['formatDate(parseDate("2026-02-02"), "Do")', "2nd"],
  ['formatDate(parseDate("2026-02-03"), "Do")', "3rd"],
  ['formatDate(parseDate("2026-02-04"), "Do")', "4th"],
  ['formatDate(parseDate("2026-02-11"), "Do")', "11th"],
  ['formatDate(parseDate("2026-02-12"), "Do")', "12th"],
  ['formatDate(parseDate("2026-02-13"), "Do")', "13th"],
  ['formatDate(parseDate("2026-02-21"), "Do")', "21st"],
  ['formatDate(parseDate("2026-02-22"), "Do")', "22nd"],
  ['formatDate(parseDate("2026-01-23"), "Do")', "23rd"],
  ['formatDate(parseDate("2026-01-31"), "Do")', "31st"],
  ['formatDate(parseDate("2026-12-01"), "Q")', "4"],
  ['formatDate(parseDate("2026-06-30"), "Q")', "2"],
  ['formatDate(parseDate("0099-01-01"), "YYYY YY")', "0099 99"],
  ['parseDate("2026-02-30")', null],
  ['parseDate("2026-13-01")', null],
  ['parseDate("26-01-01")', null],
  ['parseDate("2026-01-01T25:00")', null],
  ['parseDate("2026-01-01T10:60")', null],
  ['parseDate("2026-01-01T10:30:60")', null],
  ['formatDate(parseDate("2026-01-01 10:30:15"), "HH:mm:ss")', "10:30:15"],
  ['formatDate(parseDate("2026-01-01T10:30"), "HH:mm")', "10:30"],
  ['parseDate(" 2026-01-01")', null],
  ['parseDate("2026-01-01 ")', null],
  ['parseDate("2026-01-01\\n")', null],
  ['parseDate("0000-01-01")', null],
  ['parseDate("2026-1-1")', null],
  ['parseDate("2026-01-01T10")', null],
  ['parseDate("2024-02-29").year()', 2024],
  ['parseDate("2023-02-29")', null],
  ['parseDate("1900-02-29")', null],
  ['parseDate("2000-02-29").year()', 2000],
  ['parseDate("")', null],
  ['timestamp(parseDate("2026-10-18T11:30"))', Date.UTC(2026, 9, 18, 9, 30)],
  [`formatDate(fromTimestamp(${String(Date.UTC(2026, 9, 18, 9, 30))}), "YYYY-MM-DD HH:mm")`, "2026-10-18 11:30"],
  ['toNumber(parseDate("1970-01-01T02:00"))', 0],
  ["fromTimestamp(-1).year()", 1970],
  ["fromTimestamp(1e300)", null],
  ["fromTimestamp(0.9).minute()", 0],
]);

table("lists", [
  ["at([1, 2, 3], 5)", null],
  ["at([1, 2, 3], -4)", null],
  ["at([1, 2, 3], 1.9)", 2],
  ["at([1, 2, 3], -1.9)", 3],
  ["at([1, 2, 3], at([1], 3))", null],
  ["first([])", null],
  ["first([4, 5])", 4],
  ["last([1, 2])", 2],
  ["last([])", null],
  ["slice([1, 2, 3, 4], -2)", [3, 4]],
  ["slice([1, 2, 3], 1, 1)", null],
  ["slice([1, 2, 3], 5)", null],
  ["slice([1, 2, 3], -10, 2)", [1, 2]],
  ["slice([1, 2, 3], 0.9, 2.9)", [1, 2]],
  ["concat([1], [2, 3], [])", [1, 2, 3]],
  ["concat([], [1])", [1]],
  ['concat(prop("Tags"), ["green"])', ["red", "blue", "green"]],
  ['sort(["b", "a", "C"])', ["C", "a", "b"]],
  ["sort([3, 1, 2])", [1, 2, 3]],
  ["sort([true, false, true])", [false, true, true]],
  ["sort([parseDate(\"2026-02-01\"), parseDate(\"2026-01-01\")]).map(month(current))", [1, 2]],
  ["sort([3, at([1], 5), 2]).map(format(current)).join(\"|\")", "2|3|"],
  ["sort([\"😀\", \"ｱ\"])", ["ｱ", "😀"]],
  ["reverse([1, 2, 3])", [3, 2, 1]],
  ["unique([1, 1, 2, 1])", [1, 2]],
  ['unique(["a", "A", "a"])', ["a", "A"]],
  ["unique([1, at([1], 5), at([1], 5)]).length()", 2],
  ["unique([0, -0, 1])", [0, 1]],
  ['includes(["a"], "a")', true],
  ["includes([1, 2], 3)", false],
  ["includes([], 1)", false],
  ['includes(prop("Tags"), "red")', true],
  ["includes([1, at([1], 5)], at([1], 5))", true],
  ["flat([1, 2])", [1, 2]],
  ["[1, 2, 3].filter(index > 0)", [2, 3]],
  ["[1, 2, 3].filter(current > 5)", null],
  ["[1, 2].map(current * 10 + index)", [10, 21]],
  ["[1, 2].map([10, 20].map(current).sum() + current)", [31, 32]],
  ["[1, 2].map(let(x, current, [10, 20].map(current + x).sum()))", [32, 34]],
  ['["a", "b"].find(current == "b")', "b"],
  ['["a", "b"].find(current == "z")', null],
  ['["a", "b"].findIndex(current == "z")', -1],
  ['["a", "b"].findIndex(current == "b")', 1],
  ["[1, 2].some(current > 1)", true],
  ["[1, 2].some(current > 5)", false],
  ["[].some(current)", false],
  ["[1, 2].every(current > 0)", true],
  ["[1, 2].every(current > 1)", false],
  ["[1, 2, 3].count()", 3],
  ["[1, 2, 3].count(current > 1)", 2],
  ["[].count()", 0],
  ["[1, 2, 3].map(index)", [0, 1, 2]],
  ["[1, at([1], 5)].count()", 2],
  ["[[1, 2].length(), 3]", [2, 3]],
  ['prop("Tags").map(upper(current))', ["RED", "BLUE"]],
  ['prop("Tags").includes("blue")', true],
  ['prop("Tags").filter(length(current) > 3)', ["blue"]],
]);

table("logic and the rest", [
  ["if(true, 1, 2)", 1],
  ["if(false, 1, 2)", 2],
  ["if(at([true], 5), 1, 2)", 2],
  ["ifs(false, 1, false, 2, 3)", 3],
  ["ifs(false, 1, true, 2, 3)", 2],
  ["ifs(true, 1, true, 2, 3)", 1],
  ["and(true, true, false)", false],
  ["and(true, true, true)", true],
  ["or(false, false, true)", true],
  ["or(false, false, false)", false],
  ["and(at([true], 5), true)", false],
  ["or(at([true], 5), true)", true],
  ["not(at([true], 5))", true],
  ['empty("")', true],
  ["empty([])", true],
  ["empty(false)", true],
  ["empty(true)", false],
  ["empty(1)", false],
  ['empty(prop("Tags"))', false],
  ['empty(prop("Due"))', false],
  ["empty(at([1], 5))", true],
  ["equal(1, 1)", true],
  ['unequal("a", "b")', true],
  ["let(x, 1, let(x, 2, x))", 2],
  ["let(x, 1, let(y, 2, x + y))", 3],
  ["lets(a, 1, b, a + 1, b)", 2],
  ["lets(a, 1, a, a + 1, a)", 2],
  ["let(x, [1, 2], x.length())", 2],
  ["id()", "p1"],
  ['prop("Tasks").first().id()', "p2"],
  ["id(at(prop(\"Tasks\"), 7))", null],
  ['name(prop("Owner").first())', "Ada"],
  ['email(prop("Owner").first())', "ada@example.com"],
  ['name(prop("Owner").at(3))', null],
  ['prop("Tasks").map(id()).length()', 2],
]);

describe("properties", () => {
  it("reads kinds as the table of 2.3 says", () => {
    expect(value('prop("Score")')).toBe(7);
    expect(value('prop("Status")')).toBe("To do");
    expect(value('prop("Tags")')).toEqual(["red", "blue"]);
    expect(value('prop("Done")')).toBe(true);
    expect(value('prop("Note")')).toBe("Hello World 42");
    expect(value('prop("Name")')).toBe("Project");
    expect(value('prop("title")')).toBe("Project");
  });

  it("reads an unset property as empty, text as \"\", a checkbox as false, a list as []", () => {
    expect(value('prop("Score")', "p4")).toBeNull();
    expect(value('prop("Score") + 1', "p4")).toBeNull();
    expect(value('prop("Status") == ""', "p4")).toBe(true);
    expect(value('prop("Note") == ""', "p4")).toBe(true);
    expect(value('prop("Done")', "p4")).toBe(false);
    expect(value('prop("Tags").length()', "p4")).toBe(0);
    expect(value('prop("Name")', "p4")).toBeNull(); // "" is no value
    expect(value('prop("Name") == ""', "p4")).toBe(true);
    expect(value('prop("Due")', "p4")).toBeNull();
  });

  it("reads nothing from a page that is not there", () => {
    expect(value('prop("Score")', "nope")).toBeNull();
    expect(value('prop("Name")', "nope")).toBeNull();
  });

  it("types the result of `.prop` on a list of pages as an error", () => {
    const result = fx.run('prop("Tasks").prop("Hours")');
    expect(isFormulaError(result) && result.code).toBe("type");
  });

  it("reads `.prop` on a related page: stored values, title, and nothing past one hop", () => {
    expect(value('prop("Tasks").map(current.prop("Hours"))')).toEqual([5, 2.5]);
    expect(value('prop("Tasks").first().prop("Name")')).toBe("Task A");
    expect(value('prop("Tasks").first().prop("Tasks").length()')).toBe(0);
  });
});

describe("budget and limits", () => {
  it("is empty when the evaluation goes over its step budget", () => {
    const long = 'split(repeat("a", 5000), "")';
    expect(value(`${long}.map(${long}.map(current).length())`)).toBeNull();
  });

  it("runs a moderate nesting", () => {
    expect(value('split(repeat("a", 30), "").map(split(repeat("a", 30), "").count()).sum()')).toBe(900);
  });

  it("empties a text longer than 10,000 characters", () => {
    expect(value('repeat("a", 6000) + repeat("a", 6000)')).toBeNull();
    expect(value('(repeat("a", 5000) + repeat("a", 5000)).length()')).toBe(10000);
    expect(value('length(repeat("a", 6000) + repeat("a", 6000))')).toBe(0);
  });

  it("empties a list longer than 10,000 items", () => {
    expect(value('split(repeat("a", 10000), "").length()')).toBe(10000);
    expect(value('concat(split(repeat("a", 6000), ""), split(repeat("a", 6000), "")).length()')).toBe(0);
    expect(value('split(repeat("a", 6000), "")')).not.toBeNull();
  });
});

describe("the final result (4.4)", () => {
  it("has no value for empty, \"\" and []", () => {
    expect(value('""')).toBeNull();
    expect(value("[]")).toBeNull();
    expect(value("at([1], 5)")).toBeNull();
    expect(value("0")).toBe(0);
    expect(value("false")).toBe(false);
  });

  it("drops the empty items of a list result but keeps them inside", () => {
    expect(value('["a", "", "b"]')).toEqual(["a", "b"]);
    expect(value('["a", "", "b"].length()')).toBe(3);
    expect(value('["", ""]')).toBeNull();
    expect(value('[1, at([1], 5), 2]')).toEqual([1, 2]);
  });

  it("writes dates as DateValue, dropping the seconds", () => {
    const local = new Date(1760000000123 + 7200 * 1000).toISOString();
    expect(fx.run("fromTimestamp(1760000000123)")).toEqual({
      type: "date",
      data: { date: local.slice(0, 10), time: local.slice(11, 16) },
    });
    expect(fx.run('parseDate("2026-01-02")')).toEqual({ type: "date", data: { date: "2026-01-02" } });
  });

  it("never writes -0", () => {
    const result = fx.run("-0");
    expect(result).toEqual({ type: "number", data: 0 });
    expect(Object.is((result as { data: number }).data, 0)).toBe(true);
    const rounded = fx.run("round(-0.2)");
    expect(Object.is((rounded as { data: number }).data, 0)).toBe(true);
  });
});

describe("a host that reports things oddly", () => {
  const schema: PropertySpec[] = [
    { id: "n", name: "N", kind: "number" },
    { id: "s", name: "S", kind: "select", options: [{ id: "o1", name: "One" }] },
    { id: "m", name: "M", kind: "multi_select", options: [{ id: "o1", name: "One" }] },
    { id: "files", name: "Files", kind: "files" },
    { id: "check", name: "Check", kind: "checklist" },
    { id: "place", name: "Place", kind: "place" },
    { id: "created", name: "Created", kind: "created_time" },
    { id: "edited", name: "Edited", kind: "last_edited_time" },
  ];
  const pages: PageSpec[] = [
    {
      id: "a",
      title: "A",
      created: Date.UTC(2026, 0, 1, 22, 30),
      edited: Date.UTC(2026, 0, 2, 0, 15),
      values: {
        n: { kind: "text", value: "wrong kind" },
        s: { kind: "select", value: "gone" },
        m: { kind: "multi_select", value: ["gone", "o1", "gone"] },
        files: { kind: "files", value: ["f1"] },
        check: {
          kind: "checklist",
          value: [
            { id: "i1", text: "x", done: true },
            { id: "i2", text: "y", done: false },
          ],
        },
        place: { kind: "place", value: { name: "Home" } },
      },
    },
  ];
  const odd = makeFixture(schema, pages, { nowMs: Date.UTC(2026, 0, 1, 23, 30), offsetSeconds: 3600 });

  it("reads a value of another kind as unset, an unknown option as missing", () => {
    expect(plain(odd.run('prop("N")', "a"))).toBeNull();
    expect(plain(odd.run('prop("S")', "a"))).toBeNull();
    expect(plain(odd.run('prop("M")', "a"))).toEqual(["One"]);
  });

  it("reads checklists, places, files", () => {
    expect(plain(odd.run('prop("Check")', "a"))).toEqual([true, false]);
    expect(plain(odd.run('prop("Check").every(current)', "a"))).toBe(false);
    expect(plain(odd.run('prop("Place")', "a"))).toBe("Home");
    expect(plain(odd.run('prop("Files").length()', "a"))).toBe(0);
  });

  it("reads created and edited times in the local day", () => {
    expect(plain(odd.run('formatDate(prop("Created"), "YYYY-MM-DD HH:mm")', "a"))).toBe("2026-01-01 23:30");
    expect(plain(odd.run('formatDate(prop("Edited"), "YYYY-MM-DD HH:mm")', "a"))).toBe("2026-01-02 01:15");
    expect(plain(odd.run('prop("Created") < prop("Edited")', "a"))).toBe(true);
  });

  it("takes today from the local day, not the UTC day", () => {
    expect(plain(odd.run('formatDate(today(), "YYYY-MM-DD")', "a"))).toBe("2026-01-02");
    expect(plain(odd.run('formatDate(now(), "YYYY-MM-DD HH:mm")', "a"))).toBe("2026-01-02 00:30");
  });
});
