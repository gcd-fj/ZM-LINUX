package {
    import flash.display.MovieClip;

    public class ArraySortOn extends MovieClip {
        public function ArraySortOn() {
            try {
                check();
                trace("ARRAY_SORT_ON_OK");
            } catch (error:Error) {
                trace("ARRAY_SORT_ON_FAIL " + error.message);
            }
        }

        private function require(ok:Boolean, message:String):void {
            if (!ok) throw new Error(message);
        }

        private function check():void {
            // Flash's FieldCompare orders objects before primitive entries.
            // It does not read `id` (or even `length`) on primitive strings.
            var low:Object = {id:1, length:100};
            var high:Object = {id:2, length:200};
            var entries:Array = ["reward-placeholder", high, low];
            require(entries.sortOn("id", Array.NUMERIC) === entries, "in-place result");
            require(entries[0] === low && entries[1] === high && entries[2] === "reward-placeholder", "mixed ascending");
            entries.sortOn("id", Array.NUMERIC | Array.DESCENDING);
            require(entries[0] === "reward-placeholder" && entries[1] === high && entries[2] === low, "mixed descending");
            var lengths:Array = ["x", high, low];
            lengths.sortOn("length", Array.NUMERIC);
            require(lengths[0] === low && lengths[1] === high && lengths[2] === "x", "primitive length is not a sort field");

            for each (var primitive:* in ["x", 7, true, null, undefined]) {
                var pair:Array = [primitive, low];
                pair.sortOn("id", Array.NUMERIC);
                require(pair[0] === low && pair[1] === primitive, "primitive category");
            }
            var indexed:Array = ["x", high, low];
            var indices:Array = indexed.sortOn("id", Array.NUMERIC | Array.RETURNINDEXEDARRAY);
            require(indices.join(",") == "2,1,0", "indexed result");
            require(indexed[0] === "x" && indexed[1] === high && indexed[2] === low, "indexed source unchanged");
            var duplicates:Array = ["x", "y", low];
            require(duplicates.sortOn("id", Array.UNIQUESORT) === 0, "primitive entries compare equal");
            require(duplicates[0] === "x" && duplicates[1] === "y" && duplicates[2] === low, "unique source unchanged");

            var fields:Array = [{id:1, rank:1}, {id:1, rank:2}, {id:0, rank:9}];
            fields.sortOn(["id", "rank"], [Array.NUMERIC, Array.NUMERIC | Array.DESCENDING]);
            require(fields[0].id == 0 && fields[1].rank == 2 && fields[2].rank == 1, "object multi-field ordering");
            var caught:Boolean = false;
            try {
                var broken:Array = [new BrokenReward(), low];
                broken.sortOn("id");
            } catch (getterError:Error) {
                caught = getterError.message == "reward getter failure";
            }
            require(caught, "real getter exceptions must propagate");
            caught = false;
            try {
                var sealed:Array = [new SealedReward(), low];
                sealed.sortOn("id");
            } catch (sealedError:ReferenceError) {
                caught = sealedError.errorID == 1069;
            }
            require(caught, "sealed object property lookup must remain strict");
        }
    }
}

class BrokenReward {
    public function get id():int { throw new Error("reward getter failure"); }
}
class SealedReward {}
