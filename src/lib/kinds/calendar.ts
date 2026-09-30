// Something happening somewhere, on a day or at a time.
//
// NIP-52 spells the two kinds the same way apart from `start`, which is an ISO
// date on 31922 and unix seconds on 31923, so one reader answers both and says
// which it read. `@welshman/domain`'s `TimeEvent` covers 31923 alone and
// `Number`s the tag, which on a date-based one is NaN.

import {EventReader, EventWriter, KindFactory, type KindContext} from "@welshman/domain"
import {EVENT_DATE, EVENT_TIME} from "@welshman/util"

/** When something happens, and whether it has a clock time at all. */
export type Occasion = {at: number; allDay: boolean}

const occasion = (value: string | undefined): Occasion | undefined => {
  if (!value) return undefined

  if (/^\d+$/.test(value)) {
    return {at: Number(value), allDay: false}
  }

  const parsed = Date.parse(value)

  return Number.isNaN(parsed) ? undefined : {at: parsed / 1000, allDay: true}
}

export class CalendarReader extends EventReader {
  private tag(name: string) {
    return this.event.tags.find(tag => tag[0] === name)?.[1]
  }

  title(): string | undefined {
    return this.tag("title")
  }

  location(): string | undefined {
    return this.tag("location")
  }

  start(): Occasion | undefined {
    return occasion(this.tag("start"))
  }

  end(): Occasion | undefined {
    return occasion(this.tag("end"))
  }
}

export class CalendarWriter extends EventWriter<CalendarReader> {
  private details: Record<string, string> = {}

  constructor(kind: number, context: KindContext, reader?: CalendarReader) {
    super(kind, context, reader)
    this.dropTags(tag => ["title", "location", "start", "end"].includes(tag[0]))
  }

  setTitle(title: string) {
    this.details.title = title

    return this
  }

  setLocation(location: string) {
    this.details.location = location

    return this
  }

  /** When it happens, written the way the kind spells it. */
  setStart(start: Occasion) {
    this.details.start = written(this.kind, start)

    return this
  }

  setEnd(end: Occasion) {
    this.details.end = written(this.kind, end)

    return this
  }

  protected renderDomainTags(): string[][] {
    return Object.entries(this.details).map(([name, value]) => [name, value])
  }
}

/** The one place the two spellings of a moment are chosen between. */
const written = (kind: number, {at}: Occasion) =>
  kind === EVENT_DATE ? new Date(at * 1000).toISOString().slice(0, 10) : String(Math.floor(at))

export const DateEvent = new KindFactory({
  kind: EVENT_DATE,
  reader: CalendarReader,
  writer: CalendarWriter,
})

export const TimeEvent = new KindFactory({
  kind: EVENT_TIME,
  reader: CalendarReader,
  writer: CalendarWriter,
})
