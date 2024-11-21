# Aviate

Aviate will eventually be a VFR flight planner. The intent is to use free FAA assets (charts and other data) without any massaging or conversion. You would simply download the assets you need and open the zip files directly.

VFR charts are updated every 56 days and can be downloaded here: https://www.faa.gov/air_traffic/flight_info/aeronav/digital_products/vfr/

NASR data (airports, class airspace boundaries, etc) is updated every 28 days and can be downloaded here: https://www.faa.gov/air_traffic/flight_info/aeronav/aero_data/NASR_Subscription/

> NOTE: Only the full NASR subscription zip file is supported.

The GUI for Aviate was originally egui but there's no path forward for mobile devices, so it was rewritten to use Godot's GUI.
