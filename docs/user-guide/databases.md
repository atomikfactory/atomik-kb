# Databases

In atomik-kb a "database" is a set of ordinary pages that you look at through **views**.
Every page can carry typed **properties** (a due date, a status, a price), and a view
block shows a group of pages with those properties as a list, table, board, calendar,
gallery, feed or dashboard. There is no separate database file: the pages stay normal pages.

## Adding a view

On any page, type `/` and choose **List view**, **Table view**, **Board view**,
**Calendar view**, **Gallery view** or **Dashboard view**. Then pick **Pages shown**:

- **Sub-pages of this page**
- **Pages tagged: ...** (a tag in the space)
- **Sub-pages of another page...**
- **Every page in this space**

A view block can hold several views of the same pages. Use **Add a view** next to the
view tabs, and rename, duplicate, reorder, change the layout of, or delete a view from its
menu. A database keeps at least one view and has a limit on how many it can hold.

## Layouts

| Layout | Use it for |
|---|---|
| **List** | A compact list; choose which properties show on each row |
| **Table** | Spreadsheet-style columns; sort, hide, reorder and resize columns; a calculation row per column (count, count not empty, date range and similar) |
| **Board** | Columns by a **Select** or **Status** property; drag cards between columns, add groups, collapse columns |
| **Calendar** | Pages placed by a date property (with an optional end date), or by created or last-edited time; drag a page to a day to set its date |
| **Gallery** | Cards with a cover or the first image; card size small, medium or large |
| **Feed** | A scrolling feed of pages with a text preview |
| **Dashboard** | Numbers, charts and other views of the pages (see below) |

Pages can open in a **side peek**, a **center peek** or as a **full page**; choose
**Open pages in** in the view's settings. **New page** (or **New** in a board column,
or **New page on** a calendar day) creates a page that already fits the view.

## Properties

Open **Properties...** from a page or a view to manage the properties of a space. Property
types:

- **Text, Number, Select, Multi-select, Status** (to-do, in progress, done groups),
  **Date, Checkbox, URL, Email, Phone**
- **Files and media, Checklist, Place** (a name and optional latitude and longitude),
  **Person** (members of the space)
- **Created time, Last edited time, Created by, Last edited by**: filled in by the app
- **Relation, Rollup, Formula** (below)

The type of a property cannot be changed after it is created. Options of select-type
properties can be renamed, coloured and deleted. A space has a built-in **Tags** property.
Deleting a property removes its values from every page, after a confirmation.

## Filters, sorts and bulk edits

- **Filter:** add conditions such as "is", "contains", "is before", "is within the past
  N days", "has", "is empty", "is checked", "links to" or "is me". Match **all filters** or
  **any filter**, and use **filter groups** (one level) to combine AND and OR. Dates can be
  relative (today, this week, next 7 days...).
- **Sort:** sort by title, creation or last edit time, or any property, with several sorts
  in order and ascending or descending.
- **Select several pages** with the checkboxes to **Edit property** for all of them,
  **Move to...**, **Duplicate** or **Move to trash**. Each bulk action offers **Undo**.
- **Export CSV** from **View actions** saves the rows of a view.

## Relations and rollups

A **Relation** property links a page to other pages of the same space. Tick
**Show on the related pages (two-way)** to add a matching property on the other side.
Relations cannot reach into another space; moving a page between spaces drops such links
(the move dialog tells you how many).

A **Rollup** summarises a property of the related pages: count, count unique, percent
empty, sum, average, median, minimum, maximum, range, earliest or latest date, percent or
count checked, checklist progress, or the original values.

## Formulas

A **Formula** property computes a value from the page's other properties and its related
pages. The syntax follows Notion's formulas 2.0: read a property with `prop("Name")`, and
combine it with operators and functions, for example:

```
if(prop("Done"), "Finished", format(dateBetween(prop("Due"), today(), "days")) + " days left")
```

The editor shows the result type (number, text, true or false, date, page, person or list),
suggests property names and functions as you type (Tab or Enter accepts), lists all
functions in a searchable reference, and previews the result on a real page. Functions
cover logic (`if`, `ifs`, `and`, `or`, `not`, `empty`), numbers (`round`, `sum`, `mean`, ...),
text (`contains`, `replace`, `split`, `join`, ...), dates (`now`, `today`, `dateAdd`,
`dateBetween`, `formatDate`, ...), lists (`map`, `filter`, `sort`, `unique`, `count`, ...)
and pages and people (`prop`, `name`, `email`), plus `let` and `lets` to name values.
If two properties share a name, the formula reads the first one and warns you.

## Dashboards

A **Dashboard** view is a grid of widgets over the pages of the database:

- **Number:** a count, sum, average, median, minimum or maximum of a column.
- **Chart:** a **bar**, **line** or **donut** chart of a measure grouped by a property,
  created time or last-edited time (dates can be bucketed by day, week, month, quarter or
  year).
- **View:** an embedded view of the same database (not another dashboard).

Each widget can have its own filter, applied on top of the dashboard's filter, a width
from a quarter to the full row, and a height of small, medium or large. A dashboard holds
a limited number of widgets.
