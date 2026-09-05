package {
 import flash.display.MovieClip;
 public class TimelineLabels extends MovieClip {
  public function TimelineLabels() {
   var button:MovieClip = new ButtonTimeline(); addChild(button);
   for each (var frame:int in [1,4,5,6,4,5,4,1,2,3,1]) {
    button.gotoAndStop(frame);
    if (button.getChildAt(button.numChildren-1).name != "caption") { trace("LABEL_FAIL frame="+frame+" count="+button.numChildren+" names="+button.getChildAt(0).name+","+button.getChildAt(1).name); return; }
   }
   trace("JSON_NUMBERS_OK");
  }
 }
}