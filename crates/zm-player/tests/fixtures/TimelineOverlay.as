package {
    import flash.display.MovieClip;
    import flash.display.Sprite;
    public class TimelineOverlay extends MovieClip {
        public function TimelineOverlay() {
            var button:MovieClip = new ButtonTimeline();
            addChild(button);
            button.gotoAndStop(1);
            var label:Sprite = new Sprite();
            label.name = "label";
            button.addChild(label);
            for each (var frame:int in [2,3,1,2,1,3,1]) {
                button.gotoAndStop(frame);
                if (label.parent !== button || button.getChildIndex(label) != button.numChildren - 1) {
                    trace("OVERLAY_FAIL frame=" + frame);
                    return;
                }
            }
            trace("JSON_NUMBERS_OK");
        }
    }
}
